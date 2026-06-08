//! Implements `gc` by tracing reachable objects, pruning old unreachable loose objects,
//! and cleaning stale pack sidecar files without rewriting valid packs.

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, NaiveDate};
use clap::Parser;
use git_internal::{
    errors::GitError,
    hash::{ObjectHash, get_hash_kind},
    internal::object::{
        ObjectTrait,
        commit::Commit,
        tag::Tag as GitTag,
        tree::{Tree, TreeItemMode},
        types::ObjectType,
    },
};
use ring::rand::{SecureRandom, SystemRandom};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, QueryResult, Statement,
};
use serde::{Deserialize, Serialize};

use crate::{
    command::verify_pack,
    internal::{
        config::ConfigKv,
        db::get_db_conn_instance,
        model::{object_index, reference, reflog},
        reflog::{
            ExpireOptions, Reflog as ReflogStore, ReflogError, expire_defaults_with_conn,
            expire_reflog,
        },
    },
    utils::{
        client_storage::ClientStorage,
        error::{CliError, CliResult, StableErrorCode, emit_warning},
        output::{OutputConfig, emit_json_data},
        path, util,
    },
};

const GC_EXAMPLES: &str = "\
EXAMPLES:
    libra gc                         Trace reachable objects and prune old unreachable loose objects
    libra gc --dry-run --prune=now   Report every object and stale pack file that would be removed
    libra gc --prune=now             Remove unreachable loose objects immediately
    libra --json gc --prune=never    Inspect reachability and pack hygiene without deleting objects";

const DEFAULT_PRUNE: &str = "2.weeks.ago";
const GITLINK_INDEX_MODE: u32 = 0o160000;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const SECONDS_PER_WEEK: u64 = 7 * SECONDS_PER_DAY;
const SECONDS_PER_MONTH: u64 = 30 * SECONDS_PER_DAY;
const SECONDS_PER_YEAR: u64 = 365 * SECONDS_PER_DAY;
const GC_LOCK_READ_LIMIT: u64 = 4096;

/// Command-line options for `libra gc`.
#[derive(Parser, Debug)]
#[command(after_help = GC_EXAMPLES)]
pub struct GcArgs {
    /// Do not remove anything; print/report planned actions only.
    #[arg(short = 'n', long)]
    pub dry_run: bool,

    /// Prune unreachable loose objects older than DATE (`now`, `never`, `N.days.ago`, `N.weeks.ago`, etc.).
    #[arg(long, default_value = DEFAULT_PRUNE, value_name = "DATE", conflicts_with = "no_prune")]
    pub prune: String,

    /// Do not prune unreachable loose objects or stale pack sidecars.
    #[arg(long)]
    pub no_prune: bool,

    /// Accepted for Git compatibility. Libra gc currently does not repack or delta-compress.
    #[arg(long)]
    pub aggressive: bool,

    /// Accepted for Git compatibility. Libra still performs a deterministic local pass.
    #[arg(long)]
    pub auto: bool,

    /// Force the run if a stale gc lock file is present.
    #[arg(long)]
    pub force: bool,
}

/// Complete `gc` result used by human and JSON renderers.
#[derive(Debug, Clone, Serialize)]
struct GcOutput {
    /// Effective prune option reported to callers.
    prune: String,
    /// Whether this run only reported planned removals.
    dry_run: bool,
    /// Aggregate loose-object scan and prune statistics.
    loose_objects: LooseObjectStats,
    /// Number of objects marked reachable after graph traversal.
    reachable_objects: usize,
    /// Per-object actions for unreachable loose objects.
    unreachable_objects: Vec<GcObjectAction>,
    /// Pack-directory verification and cleanup statistics.
    pack_files: PackStats,
    /// Reflog expiration statistics collected before reachability tracing.
    reflogs: ReflogExpireStats,
    /// Compatibility warnings emitted for accepted no-op flags.
    warnings: Vec<String>,
}

/// Aggregate reflog-expiration statistics for the GC pre-prune phase.
#[derive(Debug, Clone, Default, Serialize)]
struct ReflogExpireStats {
    /// Number of distinct reflog refs scanned.
    refs_scanned: usize,
    /// Number of reflog entries scanned.
    entries_scanned: usize,
    /// Number of reflog entries expired.
    pruned: usize,
    /// Number of surviving reflog entries rewritten.
    rewritten: usize,
}

/// Aggregate statistics for loose-object scanning and pruning.
#[derive(Debug, Clone, Default, Serialize)]
struct LooseObjectStats {
    /// Number of loose object files scanned.
    scanned: usize,
    /// Number of scanned loose objects that were reachable.
    reachable: usize,
    /// Number of scanned loose objects that were unreachable.
    unreachable: usize,
    /// Number of unreachable loose objects deleted.
    pruned: usize,
    /// Number of unreachable loose objects retained.
    retained: usize,
}

/// One action taken or planned for an unreachable loose object.
#[derive(Debug, Clone, Serialize)]
struct GcObjectAction {
    /// Object ID of the unreachable loose object.
    oid: String,
    /// Object type reported for the loose object.
    object_type: String,
    /// Action taken or planned for the object.
    action: GcAction,
    /// Human-readable reason for the action.
    reason: String,
}

/// JSON-stable action names for loose-object pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GcAction {
    /// Object was removed.
    Pruned,
    /// Object would be removed in a non-dry-run invocation.
    WouldPrune,
    /// Object was retained.
    Retained,
}

/// Aggregate pack-directory verification and cleanup statistics.
#[derive(Debug, Clone, Default, Serialize)]
struct PackStats {
    /// Whether `.libra/objects/pack` exists.
    directory_exists: bool,
    /// Number of valid pack/index pairs verified.
    packs_verified: usize,
    /// Number of indexed objects found in verified packs.
    objects_in_packs: usize,
    /// Actions for stale pack sidecar files.
    stale_files: Vec<PackFileAction>,
}

/// One action taken or planned for a stale pack-directory file.
#[derive(Debug, Clone, Serialize)]
struct PackFileAction {
    /// Filesystem path reported for the pack sidecar.
    path: String,
    /// Action taken or planned for the file.
    action: PackAction,
    /// Human-readable reason for the action.
    reason: String,
}

/// JSON-stable action names for pack sidecar cleanup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PackAction {
    /// File was removed.
    Pruned,
    /// File would be removed in a non-dry-run invocation.
    WouldPrune,
    /// File was retained.
    Retained,
}

/// Effective pruning policy after resolving CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrunePolicy {
    /// Never remove unreachable objects or stale pack files.
    Never,
    /// Remove entries whose modification time is at or before the cutoff.
    OlderThan(SystemTime),
}

/// Loose object discovered in the object database.
#[derive(Debug, Clone)]
struct LooseObject {
    /// Object ID reconstructed from the loose-object path.
    hash: ObjectHash,
    /// Filesystem path to the loose-object file.
    path: PathBuf,
}

/// Mutable state used while collecting and tracing object reachability.
#[derive(Debug, Clone, Default)]
struct Reachability {
    /// Loose objects discovered before graph traversal.
    loose: Vec<LooseObject>,
    /// Root object IDs loaded from refs, reflogs, and index entries.
    roots: HashSet<ObjectHash>,
    /// Object IDs protected from pruning without being part of Git reachability.
    protected: HashSet<ObjectHash>,
    /// Object IDs reached by graph traversal.
    reachable: HashSet<ObjectHash>,
    /// Non-fatal graph traversal warnings.
    warnings: Vec<String>,
}

/// Files in `.libra/objects/pack` grouped by shared pack stem.
#[derive(Debug, Clone, Default)]
struct PackGroup {
    /// Matching `.pack` file when present.
    pack: Option<PathBuf>,
    /// Matching `.idx` file when present.
    idx: Option<PathBuf>,
    /// Matching `.keep` file when present.
    keep: Option<PathBuf>,
    /// Other files sharing the same pack stem.
    others: Vec<PathBuf>,
}

/// Held while a `gc` process owns `.libra/gc.lock`.
struct GcLockGuard {
    /// Path to the lock file that should be removed on drop.
    path: PathBuf,
    /// Random owner token written to the lock file for safe cleanup.
    token: String,
    /// Open handle keeping the create-new lock file alive for this process.
    _file: fs::File,
    /// Whether `--force` removed an existing lock before acquisition.
    forced: bool,
}

impl Drop for GcLockGuard {
    /// Remove the repository-local lock file when the guard leaves scope.
    fn drop(&mut self) {
        if read_gc_lock_token(&self.path)
            .ok()
            .flatten()
            .is_some_and(|token| token == self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Held while a forced GC run replaces a stale `.libra/gc.lock`.
struct GcReplaceLockGuard {
    /// Path to the replacement mutex file.
    path: PathBuf,
    /// Random owner token written to the replacement mutex.
    token: String,
    /// Open handle keeping the create-new mutex file alive for this process.
    _file: fs::File,
}

/// Read-only worktree entry used to validate linked-worktree `.libra` symlinks.
#[derive(Deserialize)]
struct GcWorktreeEntry {
    /// Canonical absolute worktree path persisted by `worktree add`.
    path: String,
    /// Whether this entry represents the main worktree.
    is_main: bool,
}

/// Read-only worktree state used before GC acquires any lock.
#[derive(Deserialize)]
struct GcWorktreeState {
    /// Worktrees registered for a shared `.libra` storage directory.
    worktrees: Vec<GcWorktreeEntry>,
}

impl Drop for GcReplaceLockGuard {
    /// Remove the replacement mutex only if this guard still owns it.
    fn drop(&mut self) {
        if read_gc_lock_token(&self.path)
            .ok()
            .flatten()
            .is_some_and(|token| token == self.token)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Run `gc` with default human-output configuration.
pub async fn execute(args: GcArgs) -> Result<(), String> {
    execute_safe(args, &OutputConfig::default())
        .await
        .map_err(|err| err.render())
}

/// # Side Effects
///
/// Reads refs, reflogs, the index, loose objects, and pack sidecar files. When
/// pruning is enabled and `--dry-run` is absent, deletes only unreachable loose
/// object files and stale pack sidecars; valid pack/index pairs are never
/// rewritten or removed.
///
/// # Errors
///
/// Returns structured CLI errors for invalid prune dates, unreadable object
/// storage, corrupt reachable objects, malformed pack/index pairs, and failed
/// deletion attempts.
pub async fn execute_safe(args: GcArgs, output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;
    ensure_repository_storage_metadata()?;
    let result = run_gc(&args).await?;
    render_gc_output(&result, output)
}

/// Execute the garbage-collection pass and return renderable statistics.
async fn run_gc(args: &GcArgs) -> CliResult<GcOutput> {
    ensure_repository_storage_metadata()?;
    let lock = acquire_gc_lock(args.force)?;
    let policy = prune_policy(args)?;
    let objects_dir = path::objects();
    ensure_real_object_directory(&objects_dir)?;
    let storage = ClientStorage::local(objects_dir);
    let (reflogs, reflog_warnings) = expire_reflogs_for_gc(&storage, args.dry_run).await?;
    let mut reachability = collect_reachability(&storage).await?;
    reachability.warnings.extend(reflog_warnings);
    trace_reachable(&storage, &mut reachability);
    let skip_loose_prune = !args.dry_run && !reachability.warnings.is_empty();
    let loose_policy = if skip_loose_prune {
        PrunePolicy::Never
    } else {
        policy
    };

    let loose =
        prune_unreachable_loose_objects(&storage, &reachability, loose_policy, args.dry_run)
            .await?;
    let pack_files = clean_pack_directory(&storage, policy, args.dry_run)?;

    let loose_stats = LooseObjectStats {
        scanned: reachability.loose.len(),
        reachable: reachability
            .loose
            .iter()
            .filter(|object| reachability.reachable.contains(&object.hash))
            .count(),
        unreachable: loose.len(),
        pruned: loose
            .iter()
            .filter(|action| action.action == GcAction::Pruned)
            .count(),
        retained: loose
            .iter()
            .filter(|action| action.action == GcAction::Retained)
            .count(),
    };

    let mut warnings = reachability.warnings.clone();
    if skip_loose_prune {
        warnings.push(
            "reachability traversal was incomplete; loose-object pruning was skipped".to_string(),
        );
    }
    if args.aggressive {
        warnings.push(
            "--aggressive is accepted for compatibility; Libra gc does not repack objects yet"
                .to_string(),
        );
    }
    if args.auto {
        warnings
            .push("--auto is accepted for compatibility; Libra still runs one local pass".into());
    }
    if args.force {
        if lock.forced {
            warnings.push("--force removed an existing gc lock before running".into());
        } else {
            warnings.push("--force is accepted for compatibility; gc lock was available".into());
        }
    }

    Ok(GcOutput {
        prune: if args.no_prune {
            "never".to_string()
        } else {
            args.prune.clone()
        },
        dry_run: args.dry_run,
        reachable_objects: reachability.reachable.len(),
        loose_objects: loose_stats,
        unreachable_objects: loose,
        pack_files,
        reflogs,
        warnings,
    })
}

/// Expire all reflogs using Git-style default `gc.reflog*` policy before pruning.
async fn expire_reflogs_for_gc(
    storage: &ClientStorage,
    dry_run: bool,
) -> CliResult<(ReflogExpireStats, Vec<String>)> {
    ensure_regular_repository_database()?;
    let db = get_db_conn_instance().await;
    let refs = reflog_ref_names(&db).await?;
    if refs.is_empty() {
        return Ok((ReflogExpireStats::default(), Vec::new()));
    }

    let (expire, expire_unreachable) = expire_defaults_with_conn(&db)
        .await
        .map_err(map_gc_reflog_error)?;
    let (tips, entries_scanned) = reflog_tips_for_refs(&db, &refs).await?;
    let mut stats = ReflogExpireStats {
        refs_scanned: refs.len(),
        entries_scanned,
        ..Default::default()
    };
    if let Some(warning) = reflog_parent_traversal_warning(storage, &tips) {
        return Ok((stats, vec![warning]));
    }

    let options = ExpireOptions {
        expire,
        expire_unreachable,
        rewrite: false,
        updateref: false,
        stale_fix: false,
        dry_run,
    };

    for ref_name in refs {
        let result = expire_reflog(
            &db,
            &ref_name,
            &options,
            gc_load_commit_parents,
            gc_oid_is_commit,
        )
        .await
        .map_err(map_gc_reflog_error)?;
        stats.pruned += result.pruned;
        stats.rewritten += result.rewritten;
    }
    Ok((stats, Vec::new()))
}

/// Enumerate reflog refs for the GC pre-prune expiration pass.
async fn reflog_ref_names<C: ConnectionTrait>(db: &C) -> CliResult<Vec<String>> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT DISTINCT ref_name FROM reflog;".to_string(),
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to enumerate reflog refs: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    let mut refs = rows
        .iter()
        .filter_map(|row| row.try_get::<String>("", "ref_name").ok())
        .collect::<Vec<_>>();
    refs.sort();
    refs.dedup();
    Ok(refs)
}

/// Load each reflog ref's current tip and count entries before expiration.
async fn reflog_tips_for_refs<C: ConnectionTrait>(
    db: &C,
    refs: &[String],
) -> CliResult<(Vec<ObjectHash>, usize)> {
    let mut tips = Vec::new();
    let mut entries_scanned = 0;
    for ref_name in refs {
        let entries = ReflogStore::find_all(db, ref_name)
            .await
            .map_err(map_gc_reflog_error)?;
        entries_scanned += entries.len();
        let Some(entry) = entries.first() else {
            continue;
        };
        if !is_null_oid(&entry.new_oid) {
            tips.push(parse_stored_hash(&entry.new_oid, "reflog tip")?);
        }
    }
    Ok((tips, entries_scanned))
}

/// Return a warning when reflog commit-parent traversal cannot be trusted.
fn reflog_parent_traversal_warning(storage: &ClientStorage, tips: &[ObjectHash]) -> Option<String> {
    let mut seen = HashSet::new();
    let mut queue = VecDeque::from_iter(tips.iter().copied());
    while let Some(hash) = queue.pop_front() {
        if !seen.insert(hash) {
            continue;
        }
        let commit: Commit = match load_object_from_storage(storage, &hash) {
            Ok(commit) => commit,
            Err(error) => {
                return Some(format!(
                    "reflog expiration skipped because commit parent traversal is incomplete at {hash}: {error}"
                ));
            }
        };
        for parent in commit.parent_commit_ids {
            if !seen.contains(&parent) {
                queue.push_back(parent);
            }
        }
    }
    None
}

/// Load commit parents for reflog reachability expiration.
fn gc_load_commit_parents(oid: &str) -> Option<Vec<String>> {
    let hash = ObjectHash::from_str(oid).ok()?;
    let commit: Commit = load_object_for_gc(&hash).ok()?;
    Some(
        commit
            .parent_commit_ids
            .iter()
            .map(|parent| parent.to_string())
            .collect(),
    )
}

/// Return whether a reflog object ID still loads as a commit.
fn gc_oid_is_commit(oid: &str) -> bool {
    ObjectHash::from_str(oid)
        .ok()
        .is_some_and(|hash| load_object_for_gc::<Commit>(&hash).is_ok())
}

/// Convert reflog-expiration failures into GC CLI errors.
fn map_gc_reflog_error(error: ReflogError) -> CliError {
    match error {
        ReflogError::Config(detail) => CliError::fatal(format!("gc reflog expire: {detail}"))
            .with_stable_code(StableErrorCode::CliInvalidArguments),
        other => CliError::fatal(format!("gc reflog expire failed: {other}"))
            .with_stable_code(StableErrorCode::IoWriteFailed),
    }
}

/// Acquire the repository-local GC lock.
fn acquire_gc_lock(force: bool) -> CliResult<GcLockGuard> {
    let path = util::storage_path().join("gc.lock");
    let token = generate_gc_lock_token()?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            file = write_gc_lock_owner_or_cleanup(file, &path, &token)?;
            Ok(GcLockGuard {
                path,
                token,
                _file: file,
                forced: false,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && force => {
            let _replace_lock = acquire_gc_replace_lock(&path)?;
            verify_gc_lock_is_stale(&path)?;
            fs::remove_file(&path).map_err(|error| {
                CliError::fatal(format!(
                    "failed to remove existing gc lock '{}': {}",
                    path.display(),
                    format_io_error(&error)
                ))
                .with_stable_code(StableErrorCode::IoWriteFailed)
            })?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|error| {
                    CliError::fatal(format!(
                        "failed to create gc lock '{}': {}",
                        path.display(),
                        format_io_error(&error)
                    ))
                    .with_stable_code(StableErrorCode::IoWriteFailed)
                })?;
            file = write_gc_lock_owner_or_cleanup(file, &path, &token)?;
            Ok(GcLockGuard {
                path,
                token,
                _file: file,
                forced: true,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(CliError::conflict(format!(
                "gc is already running; lock file '{}' exists",
                path.display()
            ))
            .with_hint("wait for the current gc to finish, or re-run with --force after verifying the lock is stale."))
        }
        Err(error) => Err(CliError::fatal(format!(
            "failed to create gc lock '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)),
    }
}

/// Acquire the short-lived mutex used while replacing a stale GC lock.
fn acquire_gc_replace_lock(lock_path: &Path) -> CliResult<GcReplaceLockGuard> {
    let path = lock_path.with_file_name("gc.lock.replace");
    let token = generate_gc_lock_token()?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                return CliError::conflict(format!(
                    "gc lock replacement is already in progress; lock file '{}' exists",
                    path.display()
                ))
                .with_hint("wait for the current --force lock replacement to finish.");
            }
            CliError::fatal(format!(
                "failed to create gc replacement lock '{}': {}",
                path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    file = write_gc_lock_owner_or_cleanup(file, &path, &token)?;
    Ok(GcReplaceLockGuard {
        path,
        token,
        _file: file,
    })
}

/// Write owner data into a newly created lock file, deleting it on failure.
fn write_gc_lock_owner_or_cleanup(
    mut file: fs::File,
    path: &Path,
    token: &str,
) -> CliResult<fs::File> {
    match write_gc_lock_owner(&mut file, token) {
        Ok(()) => Ok(file),
        Err(error) => {
            drop(file);
            let _ = fs::remove_file(path);
            Err(error)
        }
    }
}

/// Generate an ownership token for the current GC lock file.
fn generate_gc_lock_token() -> CliResult<String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| {
        CliError::fatal("failed to generate gc lock token")
            .with_stable_code(StableErrorCode::IoWriteFailed)
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Write the current process owner data into a newly created lock file.
fn write_gc_lock_owner(file: &mut fs::File, token: &str) -> CliResult<()> {
    use std::io::Write as _;

    writeln!(file, "pid={}", std::process::id())
        .and_then(|_| writeln!(file, "token={token}"))
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to write gc lock owner: {}",
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })
}

/// Verify that an existing lock does not belong to a running process.
fn verify_gc_lock_is_stale(path: &Path) -> CliResult<()> {
    #[cfg(not(unix))]
    {
        return Err(CliError::conflict(format!(
            "gc lock '{}' cannot be verified as stale on this platform",
            path.display()
        ))
        .with_hint(
            "remove the lock manually only after verifying that no gc process is running.",
        ));
    }
    #[cfg(unix)]
    {
        let Some(pid) = read_gc_lock_pid(path)? else {
            return Err(CliError::conflict(format!(
                "gc lock '{}' does not contain a valid pid and cannot be verified as stale",
                path.display()
            ))
            .with_hint(
                "remove the lock manually only after verifying that no gc process is running.",
            ));
        };
        if process_is_running(pid) {
            return Err(CliError::conflict(format!(
                "gc is already running with pid {pid}; lock file '{}' is not stale",
                path.display()
            ))
            .with_hint("wait for the running gc to finish before retrying."));
        }
        Ok(())
    }
}

/// Read the `pid=<number>` owner recorded in a GC lock file.
fn read_gc_lock_pid(path: &Path) -> CliResult<Option<u32>> {
    Ok(read_gc_lock_content(path)?
        .lines()
        .find_map(|line| line.trim().strip_prefix("pid="))
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|pid| *pid > 0))
}

/// Read the `token=<hex>` owner recorded in a GC lock file.
fn read_gc_lock_token(path: &Path) -> CliResult<Option<String>> {
    Ok(read_gc_lock_content(path)?.lines().find_map(|line| {
        line.trim()
            .strip_prefix("token=")
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .map(ToOwned::to_owned)
    }))
}

/// Read a small regular GC lock file after rejecting unsafe metadata.
fn read_gc_lock_content(path: &Path) -> CliResult<String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to inspect gc lock '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    if metadata.file_type().is_symlink() {
        return Err(CliError::conflict(format!(
            "gc lock '{}' is a symbolic link and cannot be verified as stale",
            path.display()
        ))
        .with_hint("replace symbolic links with a regular gc.lock file before using --force."));
    }
    if !metadata.file_type().is_file() {
        return Err(CliError::conflict(format!(
            "gc lock '{}' is not a regular file and cannot be verified as stale",
            path.display()
        ))
        .with_hint(
            "remove the lock manually only after verifying that no gc process is running.",
        ));
    }
    if metadata.len() > GC_LOCK_READ_LIMIT {
        return Err(CliError::conflict(format!(
            "gc lock '{}' is too large to verify safely",
            path.display()
        ))
        .with_hint(
            "remove the lock manually only after verifying that no gc process is running.",
        ));
    }
    fs::read_to_string(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read gc lock '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })
}

#[cfg(unix)]
/// Return whether `pid` appears to identify a live process.
fn process_is_running(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
/// Return whether `pid` appears to identify a live process.
fn process_is_running(_pid: u32) -> bool {
    false
}

/// Render the garbage-collection result as human text or JSON.
fn render_gc_output(result: &GcOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("gc", result, output);
    }

    for warning in &result.warnings {
        emit_warning(warning.clone());
    }

    if output.quiet {
        return Ok(());
    }

    println!(
        "Enumerating loose objects: {} scanned, {} reachable, {} unreachable.",
        result.loose_objects.scanned,
        result.loose_objects.reachable,
        result.loose_objects.unreachable
    );
    if result.reflogs.refs_scanned > 0 {
        let entry_label = if result.reflogs.pruned == 1 {
            "entry"
        } else {
            "entries"
        };
        if result.dry_run {
            println!(
                "Would expire {} reflog {} across {} ref(s).",
                result.reflogs.pruned, entry_label, result.reflogs.refs_scanned
            );
        } else {
            println!(
                "Expired {} reflog {} across {} ref(s).",
                result.reflogs.pruned, entry_label, result.reflogs.refs_scanned
            );
        }
    }

    if result.dry_run {
        let would_prune = result
            .unreachable_objects
            .iter()
            .filter(|object| object.action == GcAction::WouldPrune)
            .count();
        println!("Would prune {would_prune} loose object(s).");
    } else {
        println!("Pruned {} loose object(s).", result.loose_objects.pruned);
    }

    if result.pack_files.directory_exists {
        println!(
            "Checked {} pack(s), containing {} indexed object(s).",
            result.pack_files.packs_verified, result.pack_files.objects_in_packs
        );
        let pack_pruned = result
            .pack_files
            .stale_files
            .iter()
            .filter(|file| matches!(file.action, PackAction::Pruned | PackAction::WouldPrune))
            .count();
        if result.dry_run {
            println!("Would clean {pack_pruned} stale pack file(s).");
        } else {
            println!("Cleaned {pack_pruned} stale pack file(s).");
        }
    } else if let Some(retained) = result.pack_files.stale_files.first() {
        println!("Skipped pack cleanup: {}.", retained.reason);
    }

    Ok(())
}

/// Resolve CLI pruning flags into an effective prune policy.
fn prune_policy(args: &GcArgs) -> CliResult<PrunePolicy> {
    if args.no_prune {
        return Ok(PrunePolicy::Never);
    }
    parse_prune_date(&args.prune)
}

/// Parse Git-style relative prune dates accepted by `gc`.
fn parse_prune_date(raw: &str) -> CliResult<PrunePolicy> {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("never") {
        return Ok(PrunePolicy::Never);
    }
    if value.eq_ignore_ascii_case("now") || value.eq_ignore_ascii_case("all") {
        return Ok(PrunePolicy::OlderThan(SystemTime::now()));
    }

    if let Ok(seconds) = value.parse::<i64>() {
        return Ok(PrunePolicy::OlderThan(system_time_from_unix_seconds(
            seconds,
        )));
    }

    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Ok(PrunePolicy::OlderThan(system_time_from_unix_seconds(
            timestamp.timestamp(),
        )));
    }

    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        && let Some(timestamp) = date.and_hms_opt(0, 0, 0)
    {
        return Ok(PrunePolicy::OlderThan(system_time_from_unix_seconds(
            timestamp.and_utc().timestamp(),
        )));
    }

    let Some((amount, unit)) = value.split_once('.') else {
        return Err(invalid_prune_date(value));
    };
    let amount = amount
        .parse::<u64>()
        .map_err(|_| invalid_prune_date(value))?;
    let seconds = match unit {
        "seconds.ago" | "second.ago" => amount,
        "minutes.ago" | "minute.ago" => amount.saturating_mul(60),
        "hours.ago" | "hour.ago" => amount.saturating_mul(60 * 60),
        "days.ago" | "day.ago" => amount.saturating_mul(SECONDS_PER_DAY),
        "weeks.ago" | "week.ago" => amount.saturating_mul(SECONDS_PER_WEEK),
        "months.ago" | "month.ago" => amount.saturating_mul(SECONDS_PER_MONTH),
        "years.ago" | "year.ago" => amount.saturating_mul(SECONDS_PER_YEAR),
        _ => return Err(invalid_prune_date(value)),
    };

    Ok(PrunePolicy::OlderThan(
        SystemTime::now()
            .checked_sub(Duration::from_secs(seconds))
            .unwrap_or(SystemTime::UNIX_EPOCH),
    ))
}

/// Convert signed Unix seconds into a `SystemTime` cutoff.
fn system_time_from_unix_seconds(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

/// Build the structured CLI error used for invalid prune dates.
fn invalid_prune_date(value: &str) -> CliError {
    CliError::fatal(format!("invalid prune date '{value}'"))
        .with_stable_code(StableErrorCode::CliInvalidArguments)
        .with_hint("use 'now', 'never', a Unix timestamp, RFC3339, YYYY-MM-DD, or a relative value like '2.weeks.ago'.")
}

/// Decide whether a filesystem entry is old enough for the prune policy.
fn should_prune(path: &Path, policy: PrunePolicy) -> CliResult<bool> {
    match policy {
        PrunePolicy::Never => Ok(false),
        PrunePolicy::OlderThan(cutoff) => {
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                CliError::fatal(format!(
                    "failed to read metadata for '{}': {}",
                    path.display(),
                    format_io_error(&error)
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            if metadata.file_type().is_symlink() {
                return Ok(false);
            }
            let modified = metadata.modified().map_err(|error| {
                CliError::fatal(format!(
                    "failed to read metadata for '{}': {}",
                    path.display(),
                    format_io_error(&error)
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            Ok(modified <= cutoff)
        }
    }
}

/// Collect loose objects and root object IDs before graph traversal.
async fn collect_reachability(storage: &ClientStorage) -> CliResult<Reachability> {
    ensure_real_object_directory(storage.base_path())?;
    let loose = list_loose_objects(storage.base_path())?;
    let (roots, protected) = collect_roots_from_database().await?;
    Ok(Reachability {
        loose,
        roots,
        protected,
        reachable: HashSet::new(),
        warnings: Vec::new(),
    })
}

/// Reject raw `.libra` symlinks before repository paths are canonicalized.
fn ensure_repository_storage_metadata() -> CliResult<()> {
    let mut current = util::cur_dir();
    loop {
        let storage = current.join(util::ROOT_DIR);
        match fs::symlink_metadata(&storage) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                ensure_registered_linked_worktree_storage(&current, &storage)?;
                return Ok(());
            }
            Ok(metadata)
                if metadata.file_type().is_dir() && is_valid_repository_storage_dir(&storage) =>
            {
                return Ok(());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(CliError::fatal(format!(
                    "failed to inspect repository metadata '{}': {}",
                    storage.display(),
                    format_io_error(&error)
                ))
                .with_stable_code(StableErrorCode::IoReadFailed));
            }
        }
        if !current.pop() {
            return Ok(());
        }
    }
}

/// Validate that a `.libra` symlink belongs to a registered linked worktree.
fn ensure_registered_linked_worktree_storage(
    worktree_root: &Path,
    storage_link: &Path,
) -> CliResult<()> {
    let storage = fs::canonicalize(storage_link).map_err(|error| {
        CliError::fatal(format!(
            "failed to resolve symlink repository metadata directory '{}': {}",
            storage_link.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    if !is_valid_repository_storage_dir(&storage) {
        return Err(CliError::fatal(format!(
            "refusing to use symlink repository metadata directory '{}' because target '{}' is not a valid repository storage directory",
            storage_link.display(),
            storage.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt));
    }
    let worktree = fs::canonicalize(worktree_root).map_err(|error| {
        CliError::fatal(format!(
            "failed to resolve linked worktree '{}': {}",
            worktree_root.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    let state = read_worktree_state_read_only(&storage)?;
    let registered = state.worktrees.iter().any(|entry| {
        !entry.is_main
            && fs::canonicalize(Path::new(&entry.path))
                .map(|path| path == worktree)
                .unwrap_or(false)
    });
    if registered {
        return Ok(());
    }
    Err(CliError::fatal(format!(
        "refusing to use symlink repository metadata directory '{}' because '{}' is not a registered linked worktree",
        storage_link.display(),
        worktree.display()
    ))
    .with_stable_code(StableErrorCode::RepoCorrupt))
}

/// Read `worktrees.json` without creating, repairing, or rewriting it.
fn read_worktree_state_read_only(storage: &Path) -> CliResult<GcWorktreeState> {
    let path = storage.join("worktrees.json");
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        CliError::fatal(format!(
            "failed to inspect linked worktree registry '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(CliError::fatal(format!(
            "linked worktree registry '{}' is not a regular file",
            path.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt));
    }
    let data = fs::read(&path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read linked worktree registry '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    serde_json::from_slice(&data).map_err(|error| {
        CliError::fatal(format!(
            "failed to parse linked worktree registry '{}': {}",
            path.display(),
            error
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

/// Return whether a raw `.libra` candidate matches repository discovery markers.
fn is_valid_repository_storage_dir(storage: &Path) -> bool {
    if storage.join(util::DATABASE).exists() {
        return true;
    }
    ["objects", "info/exclude", "hooks"]
        .iter()
        .filter(|marker| storage.join(marker).exists())
        .count()
        >= 2
}

/// Ensure the object database root is a real directory before traversal.
fn ensure_real_object_directory(objects_dir: &Path) -> CliResult<()> {
    match fs::symlink_metadata(objects_dir) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CliError::fatal(format!(
            "refusing to traverse symlink object directory '{}'",
            objects_dir.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)),
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(CliError::fatal(format!(
            "object directory '{}' is not a directory",
            objects_dir.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::fatal(format!(
            "failed to inspect object directory '{}': {}",
            objects_dir.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)),
    }
}

/// List object files stored in Git-compatible loose-object directories.
fn list_loose_objects(objects_dir: &Path) -> CliResult<Vec<LooseObject>> {
    match fs::symlink_metadata(objects_dir) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Ok(Vec::new()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect object directory '{}': {}",
                objects_dir.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    }

    let mut objects = Vec::new();
    for entry in fs::read_dir(objects_dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read object directory '{}': {}",
            objects_dir.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let entry = entry.map_err(|error| {
            CliError::fatal(format!("failed to read object directory entry: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let prefix_path = entry.path();
        let prefix_metadata = fs::symlink_metadata(&prefix_path).map_err(|error| {
            CliError::fatal(format!(
                "failed to inspect loose object directory '{}': {}",
                prefix_path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        if !prefix_metadata.file_type().is_dir() {
            continue;
        }
        let Some(prefix) = prefix_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_hex_prefix(prefix) {
            continue;
        }
        for object_entry in fs::read_dir(&prefix_path).map_err(|error| {
            CliError::fatal(format!(
                "failed to read loose object directory '{}': {}",
                prefix_path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })? {
            let object_entry = object_entry.map_err(|error| {
                CliError::fatal(format!("failed to read loose object entry: {error}"))
                    .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            let path = object_entry.path();
            let object_metadata = fs::symlink_metadata(&path).map_err(|error| {
                CliError::fatal(format!(
                    "failed to inspect loose object entry '{}': {}",
                    path.display(),
                    format_io_error(&error)
                ))
                .with_stable_code(StableErrorCode::IoReadFailed)
            })?;
            if !object_metadata.file_type().is_file() {
                continue;
            }
            let Some(suffix) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let candidate = format!("{prefix}{suffix}");
            let Ok(hash) = ObjectHash::from_str(&candidate) else {
                continue;
            };
            objects.push(LooseObject { hash, path });
        }
    }
    objects.sort_by_key(|object| object.hash.to_string());
    Ok(objects)
}

/// Return whether a directory name can be a loose-object hash prefix.
fn is_hex_prefix(prefix: &str) -> bool {
    prefix.len() == 2 && prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Load root object IDs and non-reachability prune protections.
async fn collect_roots_from_database() -> CliResult<(HashSet<ObjectHash>, HashSet<ObjectHash>)> {
    ensure_regular_repository_database()?;
    let db = get_db_conn_instance().await;
    let mut roots = HashSet::new();
    let mut protected = HashSet::new();

    let refs = reference::Entity::find().all(&db).await.map_err(|error| {
        CliError::fatal(format!("failed to load references: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for entry in refs {
        if let Some(raw) = entry.commit.as_deref() {
            roots.insert(parse_stored_hash(raw, "reference")?);
        }
    }

    let reflogs = reflog::Entity::find().all(&db).await.map_err(|error| {
        CliError::fatal(format!("failed to load reflogs: {error}"))
            .with_stable_code(StableErrorCode::IoReadFailed)
    })?;
    for entry in reflogs {
        for raw in [entry.old_oid.as_str(), entry.new_oid.as_str()] {
            if !is_null_oid(raw) {
                roots.insert(parse_stored_hash(raw, "reflog")?);
            }
        }
    }

    roots.extend(stash_roots()?);
    roots.extend(index_roots()?);
    roots.extend(rebase_state_roots(&db).await?);
    roots.extend(bisect_state_roots(&db).await?);
    roots.extend(merge_state_roots()?);
    protected.extend(object_index_prune_protections(&db).await?);
    roots.extend(agent_checkpoint_roots(&db).await?);
    Ok((roots, protected))
}

/// Check whether an optional SQLite table exists without creating it.
async fn table_exists<C: ConnectionTrait>(db: &C, table: &str) -> CliResult<bool> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
            [table.into()],
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to inspect database schema: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    Ok(row.is_some())
}

/// Add a required hash to a root set, rejecting empty or null placeholders.
fn insert_required_root(
    roots: &mut HashSet<ObjectHash>,
    raw: String,
    source: &str,
) -> CliResult<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_null_oid(trimmed) {
        return Err(
            CliError::fatal(format!("{source} is missing a valid object id"))
                .with_stable_code(StableErrorCode::RepoCorrupt),
        );
    }
    roots.insert(parse_stored_hash(trimmed, source)?);
    Ok(())
}

/// Add an optional hash that must be valid when the field is present.
fn insert_present_root(
    roots: &mut HashSet<ObjectHash>,
    raw: Option<String>,
    source: &str,
) -> CliResult<()> {
    if let Some(value) = raw {
        insert_required_root(roots, value, source)?;
    }
    Ok(())
}

/// Add newline-separated object IDs to a root set.
fn insert_line_roots(roots: &mut HashSet<ObjectHash>, raw: &str, source: &str) -> CliResult<()> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            insert_required_root(roots, trimmed.to_string(), source)?;
        }
    }
    Ok(())
}

/// Decode a required text root field and fail closed on schema corruption.
fn required_root_text(row: &QueryResult, index: usize, source: &str) -> CliResult<String> {
    row.try_get_by_index(index).map_err(|error| {
        CliError::fatal(format!("failed to decode {source}: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

/// Decode an optional text root field and fail closed on non-NULL decode errors.
fn optional_root_text(row: &QueryResult, index: usize, source: &str) -> CliResult<Option<String>> {
    row.try_get_by_index(index).map_err(|error| {
        CliError::fatal(format!("failed to decode {source}: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

/// Load object IDs held by an in-progress rebase state row.
async fn rebase_state_roots<C: ConnectionTrait>(db: &C) -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    if !table_exists(db, "rebase_state").await? {
        return legacy_rebase_state_roots();
    }
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT onto, orig_head, current_head, todo, done, stopped_sha FROM rebase_state"
                .to_string(),
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load rebase_state roots: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    for row in rows {
        insert_required_root(
            &mut roots,
            required_root_text(&row, 0, "rebase_state.onto")?,
            "rebase_state.onto",
        )?;
        insert_required_root(
            &mut roots,
            required_root_text(&row, 1, "rebase_state.orig_head")?,
            "rebase_state.orig_head",
        )?;
        insert_required_root(
            &mut roots,
            required_root_text(&row, 2, "rebase_state.current_head")?,
            "rebase_state.current_head",
        )?;
        let todo = required_root_text(&row, 3, "rebase_state.todo")?;
        let done = required_root_text(&row, 4, "rebase_state.done")?;
        insert_line_roots(&mut roots, &todo, "rebase_state.todo")?;
        insert_line_roots(&mut roots, &done, "rebase_state.done")?;
        insert_present_root(
            &mut roots,
            optional_root_text(&row, 5, "rebase_state.stopped_sha")?,
            "rebase_state.stopped_sha",
        )?;
    }
    roots.extend(legacy_rebase_state_roots()?);
    Ok(roots)
}

/// Load root object IDs from the legacy `.libra/rebase-merge` directory.
fn legacy_rebase_state_roots() -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    let dir = util::storage_path().join("rebase-merge");
    match fs::symlink_metadata(&dir) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() => {
            return Err(CliError::fatal(format!(
                "legacy rebase state '{}' is not a regular directory",
                dir.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(roots),
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect legacy rebase state '{}': {}",
                dir.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    }

    insert_required_root(
        &mut roots,
        read_required_metadata_file(&dir.join("onto"), "legacy rebase onto")?,
        "legacy rebase onto",
    )?;
    insert_required_root(
        &mut roots,
        read_required_metadata_file(&dir.join("orig-head"), "legacy rebase orig-head")?,
        "legacy rebase orig-head",
    )?;
    insert_required_root(
        &mut roots,
        read_required_metadata_file(&dir.join("current-head"), "legacy rebase current-head")?,
        "legacy rebase current-head",
    )?;
    if let Some(todo) = read_optional_metadata_file(&dir.join("todo"), "legacy rebase todo")? {
        insert_line_roots(&mut roots, &todo, "legacy rebase todo")?;
    }
    if let Some(done) = read_optional_metadata_file(&dir.join("done"), "legacy rebase done")? {
        insert_line_roots(&mut roots, &done, "legacy rebase done")?;
    }
    insert_present_root(
        &mut roots,
        read_optional_metadata_file(&dir.join("stopped-sha"), "legacy rebase stopped-sha")?,
        "legacy rebase stopped-sha",
    )?;
    Ok(roots)
}

/// Load object IDs held by bisect state.
async fn bisect_state_roots<C: ConnectionTrait>(db: &C) -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    if !table_exists(db, "bisect_state").await? {
        return Ok(roots);
    }
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT orig_head, bad, good, current, skipped FROM bisect_state".to_string(),
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load bisect_state roots: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    for row in rows {
        insert_required_root(
            &mut roots,
            required_root_text(&row, 0, "bisect_state.orig_head")?,
            "bisect_state.orig_head",
        )?;
        insert_present_root(
            &mut roots,
            optional_root_text(&row, 1, "bisect_state.bad")?,
            "bisect_state.bad",
        )?;
        let good_json = required_root_text(&row, 2, "bisect_state.good")?;
        let skipped_json = optional_root_text(&row, 4, "bisect_state.skipped")?;
        insert_required_json_roots(&mut roots, &good_json, "bisect_state.good")?;
        if let Some(skipped) = skipped_json {
            insert_optional_json_roots(&mut roots, &skipped, "bisect_state.skipped")?;
        }
        insert_present_root(
            &mut roots,
            optional_root_text(&row, 3, "bisect_state.current")?,
            "bisect_state.current",
        )?;
    }
    Ok(roots)
}

/// Add object IDs from an optional JSON string array to a root set.
fn insert_optional_json_roots(
    roots: &mut HashSet<ObjectHash>,
    raw: &str,
    source: &str,
) -> CliResult<()> {
    if raw.trim().is_empty() {
        return Ok(());
    }
    insert_required_json_roots(roots, raw, source)
}

/// Add object IDs from a required JSON string array to a root set.
fn insert_required_json_roots(
    roots: &mut HashSet<ObjectHash>,
    raw: &str,
    source: &str,
) -> CliResult<()> {
    if raw.trim().is_empty() {
        return Err(
            CliError::fatal(format!("{source} is missing a JSON object list"))
                .with_stable_code(StableErrorCode::RepoCorrupt),
        );
    }
    let hashes: Vec<String> = serde_json::from_str(raw).map_err(|error| {
        CliError::fatal(format!("invalid {source} object list: {error}"))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    for hash in hashes {
        insert_required_root(roots, hash, source)?;
    }
    Ok(())
}

/// Minimal merge-state fields that can reference protected commits.
#[derive(serde::Deserialize)]
struct MergeStateRoots {
    /// Commit checked out before the merge started.
    orig_head: Option<String>,
    /// Commit being merged into the current branch.
    target: Option<String>,
    /// Merge base recorded for conflict recovery.
    base: Option<String>,
}

/// Load object IDs from an in-progress merge state file.
fn merge_state_roots() -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    let path = util::storage_path().join("merge-state.json");
    let Some(content) = read_optional_metadata_file(&path, "merge state")? else {
        return Ok(roots);
    };
    let state: MergeStateRoots = serde_json::from_str(&content).map_err(|error| {
        CliError::fatal(format!(
            "failed to parse merge state '{}': {error}",
            path.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    insert_present_root(&mut roots, state.orig_head, "merge-state.orig_head")?;
    insert_present_root(&mut roots, state.target, "merge-state.target")?;
    insert_present_root(&mut roots, state.base, "merge-state.base")?;
    Ok(roots)
}

/// Resolve the current repository ID used by object-index rows.
async fn current_repo_id<C: ConnectionTrait>(db: &C) -> CliResult<String> {
    let value = ConfigKv::get_with_conn(db, "libra.repoid")
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to read libra.repoid: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?
        .map(|entry| entry.value)
        .unwrap_or_else(|| "unknown-repo".to_string());
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok("unknown-repo".to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

/// Return whether cloud backup is configured for this repository.
async fn cloud_backup_enabled<C: ConnectionTrait>(db: &C) -> CliResult<bool> {
    if let Some(storage_type) = config_value(db, "vault.env.LIBRA_STORAGE_TYPE").await?
        && matches!(storage_type.trim(), "s3" | "r2")
    {
        return Ok(true);
    }
    if matches!(
        std::env::var("LIBRA_STORAGE_TYPE").ok().as_deref(),
        Some("s3" | "r2")
    ) {
        return Ok(true);
    }

    for key in [
        "LIBRA_D1_ACCOUNT_ID",
        "LIBRA_D1_API_TOKEN",
        "LIBRA_D1_DATABASE_ID",
        "LIBRA_STORAGE_ENDPOINT",
        "LIBRA_STORAGE_ACCESS_KEY",
        "LIBRA_STORAGE_SECRET_KEY",
    ] {
        let env_present = std::env::var(key)
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        let config_present = config_value(db, &format!("vault.env.{key}"))
            .await?
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
        if env_present || config_present {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read a config value from the current repository database when present.
async fn config_value<C: ConnectionTrait>(db: &C, key: &str) -> CliResult<Option<String>> {
    if !table_exists(db, "config_kv").await? {
        return Ok(None);
    }
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "SELECT value FROM config_kv WHERE key = ? LIMIT 1",
            [key.into()],
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to read config value '{key}': {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    row.map(|row| row.try_get_by_index(0))
        .transpose()
        .map_err(|error| {
            CliError::fatal(format!("failed to decode config value '{key}': {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })
}

/// Load unsynced cloud index rows that should be retained until uploaded.
async fn object_index_prune_protections<C: ConnectionTrait>(
    db: &C,
) -> CliResult<HashSet<ObjectHash>> {
    let mut protected = HashSet::new();
    if !table_exists(db, "object_index").await? {
        return Ok(protected);
    }
    if !cloud_backup_enabled(db).await? {
        return Ok(protected);
    }
    let repo_id = current_repo_id(db).await?;
    let rows = object_index::Entity::find()
        .filter(object_index::Column::RepoId.eq(repo_id))
        .filter(object_index::Column::IsSynced.eq(0))
        .all(db)
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load object_index protections: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    for row in rows {
        protected.insert(parse_stored_hash(&row.o_id, "object_index.o_id")?);
    }
    Ok(protected)
}

/// Load objects explicitly referenced by the AI checkpoint catalog.
async fn agent_checkpoint_roots<C: ConnectionTrait>(db: &C) -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    if !table_exists(db, "agent_checkpoint").await? {
        return Ok(roots);
    }
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT parent_commit, tree_oid, metadata_blob_oid, traces_commit FROM agent_checkpoint"
                .to_string(),
        ))
        .await
        .map_err(|error| {
            CliError::fatal(format!("failed to load agent_checkpoint roots: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
    for row in rows {
        insert_present_root(
            &mut roots,
            optional_root_text(&row, 0, "agent_checkpoint.parent_commit")?,
            "agent_checkpoint.parent_commit",
        )?;
        insert_required_root(
            &mut roots,
            required_root_text(&row, 1, "agent_checkpoint.tree_oid")?,
            "agent_checkpoint.tree_oid",
        )?;
        insert_required_root(
            &mut roots,
            required_root_text(&row, 2, "agent_checkpoint.metadata_blob_oid")?,
            "agent_checkpoint.metadata_blob_oid",
        )?;
        insert_required_root(
            &mut roots,
            required_root_text(&row, 3, "agent_checkpoint.traces_commit")?,
            "agent_checkpoint.traces_commit",
        )?;
    }
    Ok(roots)
}

/// Load root object IDs from file-backed stash references and stash reflogs.
fn stash_roots() -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    let storage_path = util::storage_path();
    let stash_ref = storage_path.join("refs/stash");
    if let Some(content) = read_optional_metadata_file(&stash_ref, "stash reference")? {
        insert_required_root(&mut roots, content, "stash reference")?;
    }

    let stash_log = storage_path.join("logs/refs/stash");
    if let Some(content) = read_optional_metadata_file(&stash_log, "stash reflog")? {
        if content.trim().is_empty() {
            return Err(CliError::fatal("stash reflog is empty")
                .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        for (line_index, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                return Err(CliError::fatal(format!(
                    "corrupted stash reflog entry at line {}: empty entry",
                    line_index + 1
                ))
                .with_stable_code(StableErrorCode::RepoCorrupt));
            }
            let mut fields = line.split_whitespace();
            let old_oid = fields.next().ok_or_else(|| {
                CliError::fatal(format!(
                    "corrupted stash reflog entry at line {}: missing old object id",
                    line_index + 1
                ))
                .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            let new_oid = fields.next().ok_or_else(|| {
                CliError::fatal(format!(
                    "corrupted stash reflog entry at line {}: missing stash commit hash",
                    line_index + 1
                ))
                .with_stable_code(StableErrorCode::RepoCorrupt)
            })?;
            if !is_null_oid(old_oid) {
                roots.insert(parse_stored_hash(old_oid, "stash reflog old oid")?);
            }
            insert_required_root(&mut roots, new_oid.to_string(), "stash reflog new oid")?;
        }
    }
    Ok(roots)
}

/// Read a required repository metadata file as UTF-8 text.
fn read_required_metadata_file(path: &Path, label: &str) -> CliResult<String> {
    read_optional_metadata_file(path, label)?.ok_or_else(|| {
        CliError::fatal(format!("required {label} '{}' is missing", path.display()))
            .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

/// Read an optional repository metadata file, treating absence as empty data.
fn read_optional_metadata_file(path: &Path, label: &str) -> CliResult<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(CliError::fatal(format!(
                "refusing to read symlink {label} '{}'",
                path.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        Ok(metadata) if !metadata.file_type().is_file() => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect {label} '{}': {}",
                path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    }
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError::fatal(format!(
            "failed to read {label} '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)),
    }
}

/// Parse an object ID read from repository metadata.
fn parse_stored_hash(raw: &str, source: &str) -> CliResult<ObjectHash> {
    ObjectHash::from_str(raw).map_err(|error| {
        CliError::fatal(format!(
            "invalid {source} object id ({} bytes): {error}",
            raw.len()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })
}

/// Return whether a stored object ID is the all-zero null value.
fn is_null_oid(raw: &str) -> bool {
    raw.len() == get_hash_kind().size() * 2 && raw.bytes().all(|byte| byte == b'0')
}

/// Collect object IDs referenced by the working tree index.
fn index_roots() -> CliResult<HashSet<ObjectHash>> {
    let mut roots = HashSet::new();
    let index_path = path::index();
    match fs::symlink_metadata(&index_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(CliError::fatal(format!(
                "refusing to read symlink index '{}'",
                index_path.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(CliError::fatal(format!(
                "index '{}' is not a regular file",
                index_path.display()
            ))
            .with_stable_code(StableErrorCode::RepoCorrupt));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(roots),
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect index '{}': {}",
                index_path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    }
    let index = git_internal::internal::index::Index::load(&index_path).map_err(|error| {
        CliError::fatal(format!(
            "failed to read index '{}': {error}",
            index_path.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    for stage in 0..=3 {
        for entry in index.tracked_entries(stage) {
            if entry.mode == GITLINK_INDEX_MODE {
                continue;
            }
            roots.insert(entry.hash);
        }
    }
    Ok(roots)
}

/// Ensure the repository database is a regular file before SQLite opens it.
fn ensure_regular_repository_database() -> CliResult<()> {
    let database = path::database();
    match fs::symlink_metadata(&database) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(CliError::fatal(format!(
            "refusing to use symlink repository database '{}'",
            database.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)),
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(CliError::fatal(format!(
            "repository database '{}' is not a regular file",
            database.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(CliError::fatal(format!(
            "repository database '{}' is missing",
            database.display()
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)),
        Err(error) => Err(CliError::fatal(format!(
            "failed to inspect repository database '{}': {}",
            database.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)),
    }
}

/// Traverse the object graph from roots and mark all reachable objects.
fn trace_reachable(storage: &ClientStorage, reachability: &mut Reachability) {
    let mut queue = VecDeque::from_iter(reachability.roots.iter().copied());
    while let Some(hash) = queue.pop_front() {
        if !reachability.reachable.insert(hash) {
            continue;
        }
        match object_children(storage, &hash) {
            Ok(children) => {
                for child in children {
                    if !reachability.reachable.contains(&child) {
                        queue.push_back(child);
                    }
                }
            }
            Err(error) => {
                reachability.warnings.push(format!(
                    "skipping unreachable root expansion for {hash}: {}",
                    error.render()
                ));
            }
        }
    }
}

/// Return object IDs directly referenced by a commit, tree, tag, or blob.
fn object_children(storage: &ClientStorage, hash: &ObjectHash) -> CliResult<Vec<ObjectHash>> {
    let object_type = storage.get_object_type(hash).map_err(|error| {
        CliError::fatal(format!(
            "failed to inspect reachable object {hash}: {error}"
        ))
        .with_stable_code(StableErrorCode::RepoCorrupt)
    })?;
    match object_type {
        ObjectType::Commit => {
            let commit: Commit = load_object_from_storage(storage, hash)
                .map_err(|error| corrupt_object(hash, error))?;
            let mut children = Vec::with_capacity(commit.parent_commit_ids.len() + 1);
            children.push(commit.tree_id);
            children.extend(commit.parent_commit_ids);
            Ok(children)
        }
        ObjectType::Tree => {
            let tree: Tree = load_object_from_storage(storage, hash)
                .map_err(|error| corrupt_object(hash, error))?;
            Ok(tree
                .tree_items
                .iter()
                .filter(|item| item.mode != TreeItemMode::Commit)
                .map(|item| item.id)
                .collect())
        }
        ObjectType::Tag => {
            let tag: GitTag = load_object_from_storage(storage, hash)
                .map_err(|error| corrupt_object(hash, error))?;
            Ok(vec![tag.object_hash])
        }
        ObjectType::Blob => Ok(Vec::new()),
        _ => Ok(Vec::new()),
    }
}

/// Load and parse one object using the GC-local storage backend.
fn load_object_from_storage<T>(storage: &ClientStorage, hash: &ObjectHash) -> Result<T, GitError>
where
    T: ObjectTrait,
{
    let data = storage.get(hash)?;
    T::from_bytes(&data, *hash)
}

/// Load one object through the GC-local storage backend.
fn load_object_for_gc<T>(hash: &ObjectHash) -> Result<T, GitError>
where
    T: ObjectTrait,
{
    let storage = ClientStorage::local(path::objects());
    load_object_from_storage(&storage, hash)
}

/// Convert an object-load failure into a repository-corruption CLI error.
fn corrupt_object(hash: &ObjectHash, error: GitError) -> CliError {
    CliError::fatal(format!("failed to load reachable object {hash}: {error}"))
        .with_stable_code(StableErrorCode::RepoCorrupt)
}

/// Remove or report unreachable loose objects according to the prune policy.
async fn prune_unreachable_loose_objects(
    storage: &ClientStorage,
    reachability: &Reachability,
    policy: PrunePolicy,
    dry_run: bool,
) -> CliResult<Vec<GcObjectAction>> {
    let mut actions = Vec::new();
    let reachable = &reachability.reachable;
    for loose in &reachability.loose {
        if reachable.contains(&loose.hash) {
            continue;
        }

        let object_type = storage
            .get_object_type(&loose.hash)
            .map(|kind| kind.to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        if reachability.protected.contains(&loose.hash) {
            actions.push(GcObjectAction {
                oid: loose.hash.to_string(),
                object_type,
                action: GcAction::Retained,
                reason: "object is pending cloud backup".to_string(),
            });
            continue;
        }

        if should_prune(&loose.path, policy)? {
            let action = if dry_run {
                GcAction::WouldPrune
            } else {
                remove_file(&loose.path)?;
                remove_empty_parent_dir(&loose.path)?;
                if !storage.local_exist(&loose.hash) {
                    remove_object_index_rows(&loose.hash).await?;
                }
                GcAction::Pruned
            };
            actions.push(GcObjectAction {
                oid: loose.hash.to_string(),
                object_type,
                action,
                reason: "unreachable loose object matched prune policy".to_string(),
            });
        } else {
            actions.push(GcObjectAction {
                oid: loose.hash.to_string(),
                object_type,
                action: GcAction::Retained,
                reason: "unreachable object is newer than prune cutoff or pruning is disabled"
                    .to_string(),
            });
        }
    }
    Ok(actions)
}

/// Remove local cloud-backup index rows for an object no longer present locally.
async fn remove_object_index_rows(hash: &ObjectHash) -> CliResult<()> {
    ensure_regular_repository_database()?;
    let db = get_db_conn_instance().await;
    let repo_id = current_repo_id(&db).await?;
    object_index::Entity::delete_many()
        .filter(object_index::Column::OId.eq(hash.to_string()))
        .filter(object_index::Column::RepoId.eq(repo_id))
        .exec(&db)
        .await
        .map_err(|error| {
            CliError::fatal(format!(
                "failed to remove object_index row for pruned object {hash}: {error}"
            ))
            .with_stable_code(StableErrorCode::IoWriteFailed)
        })?;
    Ok(())
}

/// Verify valid pack groups and clean stale pack sidecar files.
fn clean_pack_directory(
    storage: &ClientStorage,
    policy: PrunePolicy,
    dry_run: bool,
) -> CliResult<PackStats> {
    let object_metadata = match fs::symlink_metadata(storage.base_path()) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect object directory '{}': {}",
                storage.base_path().display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    };
    if object_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink() || !metadata.file_type().is_dir())
    {
        return Ok(PackStats {
            directory_exists: false,
            stale_files: vec![PackFileAction {
                path: display_path(storage.base_path()),
                action: PackAction::Retained,
                reason: "object directory is not a real directory; retained without traversal"
                    .to_string(),
            }],
            ..Default::default()
        });
    }

    let pack_dir = storage.base_path().join("pack");
    let pack_metadata = match fs::symlink_metadata(&pack_dir) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(CliError::fatal(format!(
                "failed to inspect pack directory '{}': {}",
                pack_dir.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed));
        }
    };
    let mut stats = PackStats {
        directory_exists: pack_metadata.is_some(),
        ..Default::default()
    };
    let Some(metadata) = pack_metadata else {
        return Ok(stats);
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        stats.stale_files.push(PackFileAction {
            path: display_path(&pack_dir),
            action: PackAction::Retained,
            reason: "pack directory is not a real directory; retained without traversal"
                .to_string(),
        });
        return Ok(stats);
    }

    let groups = collect_pack_groups(&pack_dir)?;
    for (_stem, group) in groups {
        let has_keep = group.keep.is_some();
        let has_pack_index_pair = group.pack.is_some() && group.idx.is_some();
        match (&group.pack, &group.idx) {
            (Some(pack), Some(idx)) => match verify_pack::inspect_pack_files(idx, pack) {
                Ok(inspection) => {
                    stats.packs_verified += 1;
                    stats.objects_in_packs += inspection.object_count;
                }
                Err(error) => {
                    stats.stale_files.push(PackFileAction {
                        path: display_path(pack),
                        action: PackAction::Retained,
                        reason: format!(
                            "pack/index pair failed verification and was retained: {}",
                            error.render()
                        ),
                    });
                }
            },
            (Some(pack), None) => {
                stats.stale_files.push(PackFileAction {
                    path: display_path(pack),
                    action: PackAction::Retained,
                    reason: "pack file has no matching .idx; pack index can be rebuilt".to_string(),
                });
            }
            (None, Some(idx)) => {
                stats.stale_files.push(handle_pack_file(
                    idx,
                    policy,
                    dry_run,
                    has_keep,
                    "pack index has no matching .pack",
                )?);
            }
            (None, None) => {}
        }

        for other in group.others {
            if group.pack.is_some() || has_pack_index_pair || is_pack_transient_file(&other) {
                stats.stale_files.push(PackFileAction {
                    path: display_path(&other),
                    action: PackAction::Retained,
                    reason: "pack sidecar retained for an active or potentially active pack stem"
                        .to_string(),
                });
                continue;
            }
            stats.stale_files.push(handle_pack_file(
                &other,
                policy,
                dry_run,
                has_keep,
                "stale pack temporary or sidecar file",
            )?);
        }
    }

    Ok(stats)
}

/// Return whether a pack sidecar may belong to an in-progress pack writer.
fn is_pack_transient_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "lock" | "tmp"))
}

/// Group pack-directory files by their `pack-*` stem.
fn collect_pack_groups(pack_dir: &Path) -> CliResult<BTreeMap<String, PackGroup>> {
    let mut groups = BTreeMap::<String, PackGroup>::new();
    for entry in fs::read_dir(pack_dir).map_err(|error| {
        CliError::fatal(format!(
            "failed to read pack directory '{}': {}",
            pack_dir.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoReadFailed)
    })? {
        let entry = entry.map_err(|error| {
            CliError::fatal(format!("failed to read pack directory entry: {error}"))
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CliError::fatal(format!(
                "failed to inspect pack directory entry '{}': {}",
                path.display(),
                format_io_error(&error)
            ))
            .with_stable_code(StableErrorCode::IoReadFailed)
        })?;
        let Some(stem) = pack_stem(&path) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            groups.entry(stem).or_default().others.push(path);
            continue;
        }
        if !metadata.file_type().is_file() {
            continue;
        }
        let group = groups.entry(stem).or_default();
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("pack") => group.pack = Some(path),
            Some("idx") => group.idx = Some(path),
            Some("keep") => group.keep = Some(path),
            _ => group.others.push(path),
        }
    }
    Ok(groups)
}

/// Extract the shared `pack-*` stem from a pack-directory filename.
fn pack_stem(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if !file_name.starts_with("pack-") {
        return None;
    }
    if let Some(stem) = file_name.strip_suffix(".pack") {
        return Some(stem.to_string());
    }
    if let Some(stem) = file_name.strip_suffix(".idx") {
        return Some(stem.to_string());
    }
    if let Some(stem) = file_name.strip_suffix(".keep") {
        return Some(stem.to_string());
    }
    file_name
        .split_once('.')
        .map(|(stem, _)| stem.to_string())
        .or_else(|| Some(file_name.to_string()))
}

/// Remove, retain, or report one stale pack-directory file.
fn handle_pack_file(
    path: &Path,
    policy: PrunePolicy,
    dry_run: bool,
    has_keep: bool,
    reason: &str,
) -> CliResult<PackFileAction> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Ok(PackFileAction {
            path: display_path(path),
            action: PackAction::Retained,
            reason: format!("{reason}; symbolic links are never pruned"),
        });
    }
    if has_keep {
        return Ok(PackFileAction {
            path: display_path(path),
            action: PackAction::Retained,
            reason: format!("{reason}; matching .keep file is present"),
        });
    }
    if !should_prune(path, policy)? {
        return Ok(PackFileAction {
            path: display_path(path),
            action: PackAction::Retained,
            reason: format!("{reason}; file is newer than prune cutoff or pruning is disabled"),
        });
    }
    let action = if dry_run {
        PackAction::WouldPrune
    } else {
        remove_file(path)?;
        PackAction::Pruned
    };
    Ok(PackFileAction {
        path: display_path(path),
        action,
        reason: reason.to_string(),
    })
}

/// Delete a file and convert I/O failures into stable CLI errors.
fn remove_file(path: &Path) -> CliResult<()> {
    fs::remove_file(path).map_err(|error| {
        CliError::fatal(format!(
            "failed to remove '{}': {}",
            path.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)
    })
}

/// Remove an empty loose-object prefix directory after deleting an object.
fn remove_empty_parent_dir(path: &Path) -> CliResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    match fs::remove_dir(parent) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::fatal(format!(
            "failed to remove empty object directory '{}': {}",
            parent.display(),
            format_io_error(&error)
        ))
        .with_stable_code(StableErrorCode::IoWriteFailed)),
    }
}

/// Normalize common filesystem errors to stable human-readable text.
fn format_io_error(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound => "No such file or directory".to_string(),
        io::ErrorKind::PermissionDenied => "Permission denied".to_string(),
        _ => error.to_string(),
    }
}

/// Convert a path to the display string used in structured output.
fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use git_internal::{
        hash::{ObjectHash, get_hash_kind},
        internal::object::{
            ObjectTrait,
            blob::Blob,
            commit::Commit,
            signature::{Signature, SignatureType},
            tree::{Tree, TreeItem, TreeItemMode},
        },
    };
    use tempfile::tempdir;

    use super::*;
    use crate::{
        command::save_object_to_storage,
        utils::{output::JsonFormat, test, util},
    };

    /// Build a deterministic object hash for unit tests.
    fn test_hash(byte: u8) -> ObjectHash {
        ObjectHash::from_bytes(&vec![byte; get_hash_kind().size()])
            .expect("hash bytes should match active hash kind")
    }

    /// Return a PID value that does not appear to identify a live process.
    fn non_running_pid() -> u32 {
        (900_000..910_000)
            .find(|pid| !process_is_running(*pid))
            .expect("test host should have at least one unused pid in this high range")
    }

    /// Build a stable test signature.
    fn signature() -> Signature {
        Signature {
            signature_type: SignatureType::Author,
            name: "tester".to_string(),
            email: "tester@example.com".to_string(),
            timestamp: 1,
            timezone: "+0000".to_string(),
        }
    }

    /// Build a test commit that references a tree and optional parents.
    fn commit_with_tree(tree_id: ObjectHash, parents: Vec<ObjectHash>) -> Commit {
        Commit {
            id: test_hash(9),
            tree_id,
            parent_commit_ids: parents,
            author: signature(),
            committer: Signature {
                signature_type: SignatureType::Committer,
                ..signature()
            },
            message: "commit".to_string(),
        }
    }

    /// Save a test object through local-only storage.
    fn save_test_object<T>(object: &T, obj_id: &ObjectHash) -> Result<(), GitError>
    where
        T: ObjectTrait,
    {
        let storage = ClientStorage::local(path::objects());
        save_object_to_storage(&storage, object, obj_id)
    }

    /// Mark a test object's local cloud index row as already synced.
    async fn mark_object_index_synced(hash: ObjectHash) {
        ClientStorage::wait_for_background_tasks();
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE object_index SET is_synced = 1 WHERE o_id = ?",
            [hash.to_string().into()],
        ))
        .await
        .unwrap();
    }

    #[test]
    /// Covers `never` and immediate prune-date parsing.
    fn parse_prune_date_accepts_never_and_now() {
        assert_eq!(parse_prune_date("never").unwrap(), PrunePolicy::Never);
        assert!(matches!(
            parse_prune_date("now").unwrap(),
            PrunePolicy::OlderThan(_)
        ));
    }

    #[test]
    /// Covers week-based relative prune-date parsing.
    fn parse_prune_date_accepts_relative_weeks() {
        let PrunePolicy::OlderThan(cutoff) = parse_prune_date("2.weeks.ago").unwrap() else {
            panic!("expected cutoff");
        };
        assert!(cutoff < SystemTime::now());
    }

    #[test]
    /// Covers all supported relative prune-date units.
    fn parse_prune_date_accepts_supported_relative_units() {
        for value in [
            "1.second.ago",
            "2.seconds.ago",
            "1.minute.ago",
            "2.minutes.ago",
            "1.hour.ago",
            "2.hours.ago",
            "1.day.ago",
            "2.days.ago",
            "1.week.ago",
            "2.weeks.ago",
            "1.month.ago",
            "2.months.ago",
            "1.year.ago",
            "2.years.ago",
            "all",
        ] {
            if value == "all" {
                assert!(matches!(
                    parse_prune_date(value).unwrap(),
                    PrunePolicy::OlderThan(_)
                ));
                continue;
            }
            assert!(
                matches!(parse_prune_date(value).unwrap(), PrunePolicy::OlderThan(_)),
                "{value} should parse"
            );
        }
    }

    #[test]
    /// Covers absolute prune-date forms accepted by the help text.
    fn parse_prune_date_accepts_absolute_dates() {
        for value in ["0", "1970-01-01", "1970-01-01T00:00:00Z"] {
            let PrunePolicy::OlderThan(cutoff) = parse_prune_date(value).unwrap() else {
                panic!("expected absolute cutoff");
            };
            assert!(cutoff <= SystemTime::now());
        }
    }

    #[test]
    /// Covers rejection of non-relative prune-date text.
    fn parse_prune_date_rejects_unknown_values() {
        let error = parse_prune_date("yesterday").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }

    #[test]
    /// Covers rejection of invalid relative prune-date forms.
    fn parse_prune_date_rejects_bad_amount_and_unit() {
        for value in ["x.days.ago", "2.fortnights.ago"] {
            let error = parse_prune_date(value).unwrap_err();
            assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
        }
    }

    #[test]
    /// Covers loose-object prefix validation.
    fn is_hex_prefix_requires_two_hex_digits() {
        assert!(is_hex_prefix("ab"));
        assert!(is_hex_prefix("09"));
        assert!(!is_hex_prefix("abc"));
        assert!(!is_hex_prefix("zz"));
    }

    #[test]
    /// Covers grouping of standard pack filenames.
    fn pack_stem_groups_standard_pack_files() {
        assert_eq!(
            pack_stem(Path::new("pack-abc.pack")).as_deref(),
            Some("pack-abc")
        );
        assert_eq!(
            pack_stem(Path::new("pack-abc.idx")).as_deref(),
            Some("pack-abc")
        );
        assert_eq!(
            pack_stem(Path::new("pack-abc.keep")).as_deref(),
            Some("pack-abc")
        );
    }

    #[test]
    /// Covers ignoring filenames outside the pack namespace.
    fn pack_stem_ignores_non_pack_prefixes() {
        assert!(pack_stem(Path::new("tmp.pack")).is_none());
        assert!(pack_stem(Path::new("README")).is_none());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers loose-object scanning while ignoring the pack directory.
    async fn list_loose_objects_skips_pack_directory() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let blob = Blob::from_content("hello");
        save_test_object(&blob, &blob.id).unwrap();
        fs::create_dir_all(path::objects().join("pack")).unwrap();
        fs::write(path::objects().join("pack").join("pack-x.pack"), b"bad").unwrap();

        let objects = list_loose_objects(&path::objects()).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].hash, blob.id);
    }

    #[test]
    /// Covers loose-object scanning when the objects directory is absent.
    fn list_loose_objects_returns_empty_for_missing_directory() {
        let dir = tempdir().unwrap();
        let objects = list_loose_objects(&dir.path().join("missing")).unwrap();
        assert!(objects.is_empty());
    }

    #[cfg(unix)]
    #[test]
    /// Covers rejecting a symlinked object directory before traversal.
    fn ensure_real_object_directory_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let external = tempdir().unwrap();
        let objects = repo.path().join("objects");
        symlink(external.path(), &objects).unwrap();

        let err = ensure_real_object_directory(&objects).unwrap_err();

        assert!(err.render().contains("symlink object directory"));
    }

    #[test]
    /// Covers loose-object scanning filters for invalid entries.
    fn list_loose_objects_skips_non_files_and_invalid_names() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("loose-file"), b"x").unwrap();
        fs::create_dir(dir.path().join("zz")).unwrap();
        fs::create_dir(dir.path().join("ab")).unwrap();
        fs::create_dir(dir.path().join("ab").join("nested")).unwrap();
        fs::write(dir.path().join("ab").join("not-a-hash"), b"x").unwrap();

        let objects = list_loose_objects(dir.path()).unwrap();
        assert!(objects.is_empty());
    }

    #[cfg(unix)]
    #[test]
    /// Covers loose-object scanning refusing symlink directories and files.
    fn list_loose_objects_skips_symlink_entries() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let external = tempdir().unwrap();
        fs::create_dir(external.path().join("aa")).unwrap();
        fs::write(external.path().join("aa").join("bbbb"), b"x").unwrap();
        symlink(external.path().join("aa"), dir.path().join("aa")).unwrap();
        fs::create_dir(dir.path().join("bb")).unwrap();
        symlink(
            external.path().join("aa").join("bbbb"),
            dir.path().join("bb").join("bbbb"),
        )
        .unwrap();

        let objects = list_loose_objects(dir.path()).unwrap();

        assert!(objects.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers graph traversal through commit, tree, and blob objects.
    async fn trace_reachable_walks_commit_tree_and_blob() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("content");
        save_test_object(&blob, &blob.id).unwrap();
        let tree = Tree {
            id: test_hash(2),
            tree_items: vec![TreeItem {
                mode: TreeItemMode::Blob,
                id: blob.id,
                name: "file.txt".to_string(),
            }],
        };
        save_test_object(&tree, &tree.id).unwrap();
        let commit = commit_with_tree(tree.id, Vec::new());
        save_test_object(&commit, &commit.id).unwrap();

        let mut reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([commit.id]),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };
        trace_reachable(&storage, &mut reachability);

        assert!(reachability.reachable.contains(&commit.id));
        assert!(reachability.reachable.contains(&tree.id));
        assert!(reachability.reachable.contains(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers graph traversal ignoring gitlink tree entries.
    async fn trace_reachable_skips_gitlink_tree_items() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("content");
        save_test_object(&blob, &blob.id).unwrap();
        let gitlink = test_hash(4);
        let tree = Tree {
            id: test_hash(2),
            tree_items: vec![
                TreeItem {
                    mode: TreeItemMode::Blob,
                    id: blob.id,
                    name: "file.txt".to_string(),
                },
                TreeItem {
                    mode: TreeItemMode::Commit,
                    id: gitlink,
                    name: "submodule".to_string(),
                },
            ],
        };
        save_test_object(&tree, &tree.id).unwrap();
        let commit = commit_with_tree(tree.id, Vec::new());
        save_test_object(&commit, &commit.id).unwrap();

        let mut reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([commit.id]),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };
        trace_reachable(&storage, &mut reachability);

        assert!(reachability.reachable.contains(&blob.id));
        assert!(!reachability.reachable.contains(&gitlink));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers traversal when a root object was already marked reachable.
    async fn trace_reachable_skips_already_seen_roots() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("content");
        save_test_object(&blob, &blob.id).unwrap();
        let mut reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([blob.id]),
            protected: HashSet::new(),
            reachable: HashSet::from([blob.id]),
            warnings: Vec::new(),
        };
        trace_reachable(&storage, &mut reachability);

        assert_eq!(reachability.reachable.len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers stale roots being retained without aborting graph traversal.
    async fn trace_reachable_warns_for_missing_root() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let missing = test_hash(12);
        let mut reachability = Reachability {
            loose: Vec::new(),
            roots: HashSet::from([missing]),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };

        trace_reachable(&storage, &mut reachability);

        assert!(reachability.reachable.contains(&missing));
        assert_eq!(reachability.warnings.len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers destructive pruning being disabled when reachability is incomplete.
    async fn run_gc_skips_loose_prune_after_reachability_warning() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("keep when graph is incomplete");
        save_test_object(&blob, &blob.id).unwrap();
        let missing = test_hash(13);
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO reference (name, kind, \"commit\", remote) VALUES ('broken', 'Head', ?, NULL)",
            [missing.to_string().into()],
        ))
        .await
        .unwrap();

        let output = run_gc(&GcArgs {
            dry_run: false,
            prune: "now".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.pruned, 0);
        assert!(storage.exist(&blob.id));
        assert!(
            output
                .warnings
                .iter()
                .any(|warning| { warning.contains("loose-object pruning was skipped") })
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers dry-run pruning of unreachable loose objects.
    async fn prune_unreachable_loose_objects_respects_dry_run() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("garbage");
        save_test_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            true,
        )
        .await
        .unwrap();
        assert_eq!(actions[0].action, GcAction::WouldPrune);
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers deleting an unreachable loose object that matches the cutoff.
    async fn prune_unreachable_loose_objects_removes_matching_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("garbage");
        save_test_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .await
        .unwrap();
        assert_eq!(actions[0].action, GcAction::Pruned);
        assert!(!storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers pruning an object also removing its local cloud index row.
    async fn prune_unreachable_loose_objects_removes_object_index_row() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("indexed garbage");
        ConfigKv::set("libra.repoid", "repo-a", false)
            .await
            .unwrap();
        storage.put(&blob.id, &blob.data, blob.get_type()).unwrap();
        ClientStorage::wait_for_background_tasks();
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "UPDATE object_index SET is_synced = 1 WHERE o_id = ? AND repo_id = 'repo-a'",
            [blob.id.to_string().into()],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO object_index (o_id, o_type, o_size, repo_id, created_at, is_synced) \
             VALUES (?, 'blob', 1, 'repo-b', 1, 1)",
            [blob.id.to_string().into()],
        ))
        .await
        .unwrap();
        let before = object_index::Entity::find()
            .filter(object_index::Column::OId.eq(blob.id.to_string()))
            .filter(object_index::Column::RepoId.eq("repo-a"))
            .one(&db)
            .await
            .unwrap();
        assert!(before.is_some());

        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            protected: HashSet::new(),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };
        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .await
        .unwrap();

        assert_eq!(actions[0].action, GcAction::Pruned);
        let after = object_index::Entity::find()
            .filter(object_index::Column::OId.eq(blob.id.to_string()))
            .filter(object_index::Column::RepoId.eq("repo-a"))
            .one(&db)
            .await
            .unwrap();
        assert!(after.is_none());
        let other_repo = object_index::Entity::find()
            .filter(object_index::Column::OId.eq(blob.id.to_string()))
            .filter(object_index::Column::RepoId.eq("repo-b"))
            .one(&db)
            .await
            .unwrap();
        assert!(other_repo.is_some());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers retaining unreachable objects that are still pending cloud backup.
    async fn prune_unreachable_loose_objects_retains_cloud_protected_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("cloud pending");
        save_test_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::new(),
            protected: HashSet::from([blob.id]),
            reachable: HashSet::new(),
            warnings: Vec::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .await
        .unwrap();

        assert_eq!(actions[0].action, GcAction::Retained);
        assert_eq!(actions[0].reason, "object is pending cloud backup");
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers retaining loose objects marked reachable.
    async fn prune_unreachable_loose_objects_keeps_reachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("reachable");
        save_test_object(&blob, &blob.id).unwrap();
        let reachability = Reachability {
            loose: list_loose_objects(&path::objects()).unwrap(),
            roots: HashSet::from([blob.id]),
            protected: HashSet::new(),
            reachable: HashSet::from([blob.id]),
            warnings: Vec::new(),
        };

        let actions = prune_unreachable_loose_objects(
            &storage,
            &reachability,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .await
        .unwrap();
        assert!(actions.is_empty());
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers pruning an orphan pack index file.
    async fn clean_pack_directory_prunes_orphan_idx() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let idx = pack_dir.join("pack-deadbeef.idx");
        fs::write(&idx, b"orphan").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files.len(), 1);
        assert_eq!(stats.stale_files[0].action, PackAction::Pruned);
        assert!(!idx.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers retaining stale pack files protected by `.keep`.
    async fn clean_pack_directory_keeps_files_when_keep_exists() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let idx = pack_dir.join("pack-deadbeef.idx");
        let keep = pack_dir.join("pack-deadbeef.keep");
        fs::write(&idx, b"orphan").unwrap();
        fs::write(&keep, b"keep").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files[0].action, PackAction::Retained);
        assert!(idx.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers pack cleanup when the pack directory is missing.
    async fn clean_pack_directory_returns_empty_when_directory_missing() {
        let dir = tempdir().unwrap();
        let storage = ClientStorage::local(dir.path().join("objects"));

        let stats = clean_pack_directory(&storage, PrunePolicy::Never, false).unwrap();

        assert!(!stats.directory_exists);
        assert!(stats.stale_files.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers retaining orphan pack files and temporary sidecars conservatively.
    async fn clean_pack_directory_retains_orphan_pack_and_tmp_sidecar() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let pack = pack_dir.join("pack-deadbeef.pack");
        let sidecar = pack_dir.join("pack-feedface.tmp");
        fs::write(&pack, b"orphan").unwrap();
        fs::write(&sidecar, b"tmp").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files.len(), 2);
        let pack_action = stats
            .stale_files
            .iter()
            .find(|file| file.path.ends_with("pack-deadbeef.pack"))
            .unwrap();
        let sidecar_action = stats
            .stale_files
            .iter()
            .find(|file| file.path.ends_with("pack-feedface.tmp"))
            .unwrap();
        assert_eq!(pack_action.action, PackAction::Retained);
        assert!(pack_action.reason.contains("index can be rebuilt"));
        assert_eq!(sidecar_action.action, PackAction::Retained);
        assert!(pack.exists());
        assert!(sidecar.exists());
    }

    #[test]
    /// Covers identifying pack files that may belong to active writers.
    fn is_pack_transient_file_accepts_lock_and_tmp() {
        assert!(is_pack_transient_file(Path::new("pack-abcd.idx.lock")));
        assert!(is_pack_transient_file(Path::new("pack-abcd.pack.tmp")));
        assert!(!is_pack_transient_file(Path::new("pack-abcd.bitmap")));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers retaining all sidecars that share an orphan pack stem.
    async fn clean_pack_directory_retains_orphan_pack_sidecars() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let pack = pack_dir.join("pack-deadbeef.pack");
        let sidecar = pack_dir.join("pack-deadbeef.promisor");
        fs::write(&pack, b"orphan").unwrap();
        fs::write(&sidecar, b"metadata").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        let sidecar_action = stats
            .stale_files
            .iter()
            .find(|file| file.path.ends_with("pack-deadbeef.promisor"))
            .unwrap();
        assert_eq!(sidecar_action.action, PackAction::Retained);
        assert!(sidecar.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers pack verification failures not aborting unrelated cleanup.
    async fn clean_pack_directory_retains_bad_pack_and_continues() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let bad_pack = pack_dir.join("pack-bad.pack");
        let bad_idx = pack_dir.join("pack-bad.idx");
        let orphan_idx = pack_dir.join("pack-orphan.idx");
        fs::write(&bad_pack, b"bad pack").unwrap();
        fs::write(&bad_idx, b"bad idx").unwrap();
        fs::write(&orphan_idx, b"orphan").unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        let bad_action = stats
            .stale_files
            .iter()
            .find(|file| file.path.ends_with("pack-bad.pack"))
            .unwrap();
        let orphan_action = stats
            .stale_files
            .iter()
            .find(|file| file.path.ends_with("pack-orphan.idx"))
            .unwrap();
        assert_eq!(bad_action.action, PackAction::Retained);
        assert_eq!(orphan_action.action, PackAction::Pruned);
        assert!(bad_pack.exists());
        assert!(!orphan_idx.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    /// Covers pack cleanup refusing a symlink pack directory.
    async fn clean_pack_directory_retains_symlink_pack_directory() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let external = tempdir().unwrap();
        let external_file = external.path().join("pack-deadbeef.idx");
        fs::write(&external_file, b"idx").unwrap();
        let pack_dir = path::objects().join("pack");
        let _ = fs::remove_dir_all(&pack_dir);
        fs::create_dir_all(pack_dir.parent().unwrap()).unwrap();
        symlink(external.path(), &pack_dir).unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files[0].action, PackAction::Retained);
        assert!(external_file.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    /// Covers pack cleanup refusing a symlink object directory.
    async fn clean_pack_directory_retains_symlink_object_directory() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let external = tempdir().unwrap();
        let external_pack = external.path().join("pack");
        fs::create_dir_all(&external_pack).unwrap();
        let external_file = external_pack.join("pack-deadbeef.idx");
        fs::write(&external_file, b"idx").unwrap();
        let objects = path::objects();
        fs::remove_dir_all(&objects).unwrap();
        symlink(external.path(), &objects).unwrap();
        let storage = ClientStorage::local(path::objects());

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert!(!stats.directory_exists);
        assert_eq!(stats.stale_files[0].action, PackAction::Retained);
        assert!(external_file.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial]
    /// Covers pack cleanup retaining symlink entries inside pack directories.
    async fn clean_pack_directory_retains_symlink_pack_entry() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let pack_dir = path::objects().join("pack");
        fs::create_dir_all(&pack_dir).unwrap();
        let external = tempdir().unwrap();
        let external_file = external.path().join("outside.idx");
        fs::write(&external_file, b"idx").unwrap();
        symlink(&external_file, pack_dir.join("pack-deadbeef.idx")).unwrap();

        let stats = clean_pack_directory(
            &storage,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            false,
        )
        .unwrap();

        assert_eq!(stats.stale_files[0].action, PackAction::Retained);
        assert!(external_file.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers GC expiring reflogs with the default policy before object pruning.
    async fn expire_reflogs_for_gc_prunes_expired_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let commit = commit_with_tree(test_hash(8), Vec::new());
        save_test_object(&commit, &commit.id).unwrap();
        let old_oid = test_hash(1).to_string();
        let new_oid = commit.id.to_string();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO reflog \
             (ref_name, old_oid, new_oid, timestamp, committer_name, committer_email, action, message) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
            [
                "HEAD".into(),
                old_oid.into(),
                new_oid.into(),
                1_i64.into(),
                "tester".into(),
                "tester@example.com".into(),
                "commit".into(),
                "old entry".into(),
            ],
        ))
        .await
        .unwrap();

        let storage = ClientStorage::local(path::objects());
        let (stats, warnings) = expire_reflogs_for_gc(&storage, false).await.unwrap();
        let remaining = reflog::Entity::find().all(&db).await.unwrap();

        assert!(warnings.is_empty());
        assert_eq!(stats.refs_scanned, 1);
        assert_eq!(stats.entries_scanned, 1);
        assert_eq!(stats.pruned, 1);
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers GC dry-run reflog expiration reporting without deleting rows.
    async fn expire_reflogs_for_gc_dry_run_keeps_expired_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let commit = commit_with_tree(test_hash(8), Vec::new());
        save_test_object(&commit, &commit.id).unwrap();
        let old_oid = test_hash(3).to_string();
        let new_oid = commit.id.to_string();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO reflog \
             (ref_name, old_oid, new_oid, timestamp, committer_name, committer_email, action, message) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
            [
                "HEAD".into(),
                old_oid.into(),
                new_oid.into(),
                1_i64.into(),
                "tester".into(),
                "tester@example.com".into(),
                "commit".into(),
                "old entry".into(),
            ],
        ))
        .await
        .unwrap();

        let storage = ClientStorage::local(path::objects());
        let (stats, warnings) = expire_reflogs_for_gc(&storage, true).await.unwrap();
        let remaining = reflog::Entity::find().all(&db).await.unwrap();

        assert!(warnings.is_empty());
        assert_eq!(stats.pruned, 1);
        assert_eq!(remaining.len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers GC skipping reflog expiration when commit traversal is incomplete.
    async fn expire_reflogs_for_gc_skips_when_tip_traversal_is_incomplete() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let missing_tip = test_hash(7).to_string();
        let ancestor = test_hash(8).to_string();
        let null_oid = test_hash(0).to_string();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let older_than_unreachable_expire = now - 31 * 24 * 60 * 60;

        for (old_oid, new_oid, timestamp, message) in [
            (
                null_oid.as_str(),
                ancestor.as_str(),
                older_than_unreachable_expire,
                "ancestor entry",
            ),
            (
                ancestor.as_str(),
                missing_tip.as_str(),
                now,
                "missing packed tip",
            ),
        ] {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO reflog \
                 (ref_name, old_oid, new_oid, timestamp, committer_name, committer_email, action, message) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
                [
                    "HEAD".into(),
                    old_oid.into(),
                    new_oid.into(),
                    timestamp.into(),
                    "tester".into(),
                    "tester@example.com".into(),
                    "commit".into(),
                    message.into(),
                ],
            ))
            .await
            .unwrap();
        }

        let storage = ClientStorage::local(path::objects());
        let (stats, warnings) = expire_reflogs_for_gc(&storage, false).await.unwrap();
        let remaining = reflog::Entity::find().all(&db).await.unwrap();

        assert_eq!(stats.refs_scanned, 1);
        assert_eq!(stats.entries_scanned, 2);
        assert_eq!(stats.pruned, 0);
        assert_eq!(remaining.len(), 2);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("reflog expiration skipped"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `run_gc` retaining unreachable objects when pruning is disabled.
    async fn run_gc_prune_never_reports_retained_unreachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let blob = Blob::from_content("unreachable");
        save_test_object(&blob, &blob.id).unwrap();
        mark_object_index_synced(blob.id).await;

        let output = run_gc(&GcArgs {
            dry_run: false,
            prune: "never".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.unreachable, 1);
        assert_eq!(output.unreachable_objects[0].action, GcAction::Retained);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `run_gc` removing unreachable objects with an immediate cutoff.
    async fn run_gc_prune_now_removes_unreachable_object() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());

        let blob = Blob::from_content("unreachable");
        save_test_object(&blob, &blob.id).unwrap();
        mark_object_index_synced(blob.id).await;

        let output = run_gc(&GcArgs {
            dry_run: false,
            prune: "now".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.pruned, 1);
        assert!(!storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers object-directory validation happening before storage initialization.
    async fn run_gc_reports_invalid_object_directory_without_panic() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let objects = path::objects();
        fs::remove_dir_all(&objects).unwrap();
        fs::write(&objects, b"not a directory").unwrap();

        let error = run_gc(&GcArgs {
            dry_run: false,
            prune: "now".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("is not a directory"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers dry-run GC inspecting a missing object directory without creating it.
    async fn run_gc_dry_run_does_not_create_missing_object_directory() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let objects = path::objects();
        fs::remove_dir_all(&objects).unwrap();

        let output = run_gc(&GcArgs {
            dry_run: true,
            prune: "never".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(output.loose_objects.scanned, 0);
        assert!(!objects.exists());
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers rejecting a raw `.libra` symlink before canonicalized paths hide it.
    fn ensure_repository_storage_metadata_rejects_libra_symlink() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let external = tempdir().unwrap();
        fs::write(external.path().join(util::DATABASE), b"").unwrap();
        symlink(external.path(), repo.path().join(util::ROOT_DIR)).unwrap();
        let _guard = test::ChangeDirGuard::new(repo.path());

        let error = ensure_repository_storage_metadata().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("linked worktree registry"));
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers allowing a `.libra` symlink for a registered linked worktree.
    fn ensure_repository_storage_metadata_allows_registered_linked_worktree() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let storage = tempdir().unwrap();
        fs::write(storage.path().join(util::DATABASE), b"").unwrap();
        let repo_path = fs::canonicalize(repo.path()).unwrap();
        fs::write(
            storage.path().join("worktrees.json"),
            serde_json::json!({
                "worktrees": [
                    {
                        "path": repo_path,
                        "is_main": false,
                        "locked": false,
                        "lock_reason": null
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        symlink(storage.path(), repo.path().join(util::ROOT_DIR)).unwrap();
        let _guard = test::ChangeDirGuard::new(repo.path());

        ensure_repository_storage_metadata().unwrap();
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers rejecting a valid storage symlink when the worktree is not registered.
    fn ensure_repository_storage_metadata_rejects_unregistered_linked_worktree() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let other = tempdir().unwrap();
        let storage = tempdir().unwrap();
        fs::write(storage.path().join(util::DATABASE), b"").unwrap();
        let other_path = fs::canonicalize(other.path()).unwrap();
        fs::write(
            storage.path().join("worktrees.json"),
            serde_json::json!({
                "worktrees": [
                    {
                        "path": other_path,
                        "is_main": false,
                        "locked": false,
                        "lock_reason": null
                    }
                ]
            })
            .to_string(),
        )
        .unwrap();
        symlink(storage.path(), repo.path().join(util::ROOT_DIR)).unwrap();
        let _guard = test::ChangeDirGuard::new(repo.path());

        let error = ensure_repository_storage_metadata().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("not a registered linked worktree"));
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers failing closed when linked-worktree registration is corrupt.
    fn ensure_repository_storage_metadata_rejects_corrupt_worktree_registry() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let storage = tempdir().unwrap();
        fs::write(storage.path().join(util::DATABASE), b"").unwrap();
        fs::write(storage.path().join("worktrees.json"), b"{").unwrap();
        symlink(storage.path(), repo.path().join(util::ROOT_DIR)).unwrap();
        let _guard = test::ChangeDirGuard::new(repo.path());

        let error = ensure_repository_storage_metadata().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("linked worktree registry"));
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers invalid nested `.libra` directories not hiding parent symlink metadata.
    fn ensure_repository_storage_metadata_skips_invalid_libra_before_symlink() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let external = tempdir().unwrap();
        fs::write(external.path().join(util::DATABASE), b"").unwrap();
        symlink(external.path(), repo.path().join(util::ROOT_DIR)).unwrap();
        let sub = repo.path().join("sub");
        fs::create_dir_all(sub.join(util::ROOT_DIR)).unwrap();
        let _guard = test::ChangeDirGuard::new(&sub);

        let error = ensure_repository_storage_metadata().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("linked worktree registry"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers compatibility warnings for accepted Git flags.
    async fn run_gc_warns_for_compatibility_flags() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());

        let output = run_gc(&GcArgs {
            dry_run: true,
            prune: "never".to_string(),
            no_prune: false,
            aggressive: true,
            auto: true,
            force: true,
        })
        .await
        .unwrap();

        assert_eq!(output.warnings.len(), 3);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers the repository-local GC lock blocking concurrent runs.
    async fn acquire_gc_lock_blocks_second_holder() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock = acquire_gc_lock(false).unwrap();

        let error = match acquire_gc_lock(false) {
            Ok(_) => panic!("second lock should fail"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        drop(lock);
        assert!(!util::storage_path().join("gc.lock").exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers a stale guard not removing a replacement lock owned by another run.
    async fn gc_lock_drop_keeps_replacement_lock_with_different_token() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock = acquire_gc_lock(false).unwrap();
        let lock_path = util::storage_path().join("gc.lock");
        fs::remove_file(&lock_path).unwrap();
        fs::write(
            &lock_path,
            format!("pid={}\ntoken=replacement\n", non_running_pid()),
        )
        .unwrap();

        drop(lock);

        assert!(lock_path.exists());
        assert!(
            fs::read_to_string(&lock_path)
                .unwrap()
                .contains("token=replacement")
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `--force` serializing stale-lock replacement with a short-lived mutex.
    async fn acquire_gc_lock_force_rejects_concurrent_replacement() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, format!("pid={}\n", non_running_pid())).unwrap();
        let replace_lock = acquire_gc_replace_lock(&lock_path).unwrap();

        let error = match acquire_gc_lock(true) {
            Ok(_) => panic!("concurrent replacement should not proceed"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        assert!(lock_path.exists());
        drop(replace_lock);
        assert!(!lock_path.with_file_name("gc.lock.replace").exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `--force` replacing an existing GC lock file.
    async fn acquire_gc_lock_force_replaces_existing_file() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, format!("pid={}\n", non_running_pid())).unwrap();

        let lock = acquire_gc_lock(true).unwrap();

        assert!(lock.forced);
        assert!(lock_path.exists());
    }

    #[test]
    #[serial_test::serial]
    /// Covers owner-write failures removing newly created lock files.
    fn write_gc_lock_owner_or_cleanup_removes_partial_lock() {
        let repo = tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(test::setup_with_new_libra_in(repo.path()));
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, b"").unwrap();
        let file = fs::File::open(&lock_path).unwrap();

        let error = write_gc_lock_owner_or_cleanup(file, &lock_path, "token").unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::IoWriteFailed);
        assert!(!lock_path.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `--force` refusing to replace a lock owned by a live process.
    async fn acquire_gc_lock_force_rejects_live_pid() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, format!("pid={}\n", std::process::id())).unwrap();

        let error = match acquire_gc_lock(true) {
            Ok(_) => panic!("live lock should not be replaced"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        assert!(lock_path.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers malformed lock files being treated as unverifiable for `--force`.
    async fn acquire_gc_lock_force_rejects_malformed_lock() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, "stale").unwrap();

        let error = match acquire_gc_lock(true) {
            Ok(_) => panic!("malformed lock should not be replaced"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        assert!(lock_path.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers `--force` refusing to inspect symlink lock files.
    async fn acquire_gc_lock_force_rejects_symlink_lock() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let target = repo.path().join("outside.lock");
        fs::write(&target, format!("pid={}\n", non_running_pid())).unwrap();
        let lock_path = util::storage_path().join("gc.lock");
        symlink(&target, &lock_path).unwrap();

        let error = match acquire_gc_lock(true) {
            Ok(_) => panic!("symlink lock should not be replaced"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        assert!(target.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers `--force` refusing oversized lock files without reading them fully.
    async fn acquire_gc_lock_force_rejects_oversized_lock() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let lock_path = util::storage_path().join("gc.lock");
        fs::write(&lock_path, vec![b'x'; (GC_LOCK_READ_LIMIT + 1) as usize]).unwrap();

        let error = match acquire_gc_lock(true) {
            Ok(_) => panic!("oversized lock should not be replaced"),
            Err(error) => error,
        };

        assert_eq!(
            error.stable_code(),
            StableErrorCode::ConflictOperationBlocked
        );
        assert!(lock_path.exists());
    }

    #[test]
    /// Covers human dry-run output rendering.
    fn render_gc_output_prints_human_dry_run_summary() {
        let result = GcOutput {
            prune: "now".into(),
            dry_run: true,
            loose_objects: LooseObjectStats {
                scanned: 2,
                reachable: 1,
                unreachable: 1,
                pruned: 0,
                retained: 0,
            },
            reachable_objects: 1,
            unreachable_objects: vec![GcObjectAction {
                oid: "abc".into(),
                object_type: "blob".into(),
                action: GcAction::WouldPrune,
                reason: "old".into(),
            }],
            pack_files: PackStats {
                directory_exists: true,
                packs_verified: 1,
                objects_in_packs: 3,
                stale_files: vec![PackFileAction {
                    path: "pack/tmp".into(),
                    action: PackAction::WouldPrune,
                    reason: "tmp".into(),
                }],
            },
            reflogs: ReflogExpireStats::default(),
            warnings: vec!["compat warning".into()],
        };

        render_gc_output(&result, &OutputConfig::default()).unwrap();
    }

    #[test]
    /// Covers human pruning output rendering.
    fn render_gc_output_prints_human_pruned_summary() {
        let result = GcOutput {
            prune: "now".into(),
            dry_run: false,
            loose_objects: LooseObjectStats {
                scanned: 1,
                reachable: 0,
                unreachable: 1,
                pruned: 1,
                retained: 0,
            },
            reachable_objects: 0,
            unreachable_objects: Vec::new(),
            pack_files: PackStats {
                directory_exists: true,
                packs_verified: 0,
                objects_in_packs: 0,
                stale_files: vec![PackFileAction {
                    path: "pack/tmp".into(),
                    action: PackAction::Pruned,
                    reason: "tmp".into(),
                }],
            },
            reflogs: ReflogExpireStats::default(),
            warnings: Vec::new(),
        };

        render_gc_output(&result, &OutputConfig::default()).unwrap();
    }

    #[test]
    /// Covers quiet and JSON output rendering modes.
    fn render_gc_output_respects_quiet_and_json_modes() {
        let result = GcOutput {
            prune: "never".into(),
            dry_run: false,
            loose_objects: LooseObjectStats::default(),
            reachable_objects: 0,
            unreachable_objects: Vec::new(),
            pack_files: PackStats::default(),
            reflogs: ReflogExpireStats::default(),
            warnings: Vec::new(),
        };
        let quiet = OutputConfig {
            quiet: true,
            ..Default::default()
        };
        render_gc_output(&result, &quiet).unwrap();

        let json = OutputConfig {
            json_format: Some(JsonFormat::Compact),
            ..Default::default()
        };
        render_gc_output(&result, &json).unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers legacy string error rendering from `execute`.
    async fn execute_maps_errors_to_strings() {
        let dir = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(dir.path());
        let error = execute(GcArgs {
            dry_run: false,
            prune: "now".into(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap_err();
        assert!(error.contains("not a libra repository"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers structured missing-repository errors from `execute_safe`.
    async fn execute_safe_reports_missing_repository() {
        let dir = tempdir().unwrap();
        let _guard = test::ChangeDirGuard::new(dir.path());
        let error = execute_safe(
            GcArgs {
                dry_run: false,
                prune: "now".into(),
                no_prune: false,
                aggressive: false,
                auto: false,
                force: false,
            },
            &OutputConfig::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::RepoNotFound);
    }

    #[test]
    /// Covers path display conversion.
    fn display_path_uses_path_display() {
        assert!(display_path(Path::new("objects/pack")).contains("objects"));
    }

    #[test]
    /// Covers all-zero object ID detection.
    fn is_null_oid_requires_non_empty_zero_string() {
        let zero = "0".repeat(get_hash_kind().size() * 2);
        assert!(is_null_oid(&zero));
        assert!(!is_null_oid("0000"));
        assert!(!is_null_oid(""));
        assert!(!is_null_oid(&format!(
            "{}1",
            "0".repeat(get_hash_kind().size() * 2 - 1)
        )));
    }

    #[test]
    /// Covers required roots rejecting missing and all-zero placeholders.
    fn insert_required_root_rejects_missing_or_null_oid() {
        let mut roots = HashSet::new();
        let zero = "0".repeat(get_hash_kind().size() * 2);

        for raw in ["", "   ", zero.as_str()] {
            let error =
                insert_required_root(&mut roots, raw.to_string(), "rebase_state.onto").unwrap_err();
            assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
            assert!(error.render().contains("rebase_state.onto"));
        }
        assert!(roots.is_empty());
    }

    #[test]
    /// Covers present optional roots rejecting empty or null placeholders.
    fn insert_present_root_rejects_empty_or_null_oid() {
        let mut roots = HashSet::new();
        insert_present_root(&mut roots, None, "rebase_state.stopped_sha").unwrap();

        let empty =
            insert_present_root(&mut roots, Some("".to_string()), "rebase_state.stopped_sha")
                .unwrap_err();
        assert_eq!(empty.stable_code(), StableErrorCode::RepoCorrupt);

        let zero = "0".repeat(get_hash_kind().size() * 2);
        let null =
            insert_present_root(&mut roots, Some(zero), "rebase_state.stopped_sha").unwrap_err();
        assert_eq!(null.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(roots.is_empty());
    }

    #[test]
    /// Covers required JSON root lists rejecting missing content.
    fn insert_required_json_roots_rejects_empty_string() {
        let mut roots = HashSet::new();

        let error = insert_required_json_roots(&mut roots, "  ", "bisect_state.good").unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("bisect_state.good"));
        assert!(roots.is_empty());
    }

    #[test]
    /// Covers filesystem error normalization.
    fn format_io_error_normalizes_not_found() {
        let error = io::Error::new(io::ErrorKind::NotFound, "missing");
        assert_eq!(format_io_error(&error), "No such file or directory");
    }

    #[test]
    /// Covers default loose-object statistics.
    fn loose_object_stats_default_is_zero() {
        let stats = LooseObjectStats::default();
        assert_eq!(stats.scanned, 0);
        assert_eq!(stats.pruned, 0);
    }

    #[test]
    /// Covers default pack statistics.
    fn pack_stats_default_has_no_directory() {
        let stats = PackStats::default();
        assert!(!stats.directory_exists);
        assert!(stats.stale_files.is_empty());
    }

    #[test]
    /// Covers `--no-prune` policy precedence.
    fn prune_policy_obeys_no_prune() {
        let args = GcArgs {
            dry_run: false,
            prune: "now".into(),
            no_prune: true,
            aggressive: false,
            auto: false,
            force: false,
        };
        assert_eq!(prune_policy(&args).unwrap(), PrunePolicy::Never);
    }

    #[test]
    /// Covers invalid stored object IDs.
    fn parse_stored_hash_rejects_invalid_hash() {
        let error = parse_stored_hash("not-a-hash", "reference").unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(!error.render().contains("not-a-hash"));
    }

    #[cfg(unix)]
    #[test]
    /// Covers metadata file reads refusing symlinks before reading content.
    fn read_optional_metadata_file_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().unwrap();
        let sensitive = dir.path().join("secret");
        let link = dir.path().join("stash");
        fs::write(&sensitive, "not-a-hash-secret").unwrap();
        symlink(&sensitive, &link).unwrap();

        let error = read_optional_metadata_file(&link, "stash reference").unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(!error.render().contains("not-a-hash-secret"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers index entries being used as reachability roots.
    async fn collect_roots_includes_index_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        fs::write(repo.path().join("file.txt"), "indexed").unwrap();
        util::working_dir();
        let add = crate::command::add::AddArgs {
            pathspec: vec!["file.txt".into()],
            ..Default::default()
        };
        crate::command::add::execute_safe(add, &OutputConfig::default())
            .await
            .unwrap();

        let (roots, _protected) = collect_roots_from_database().await.unwrap();
        assert!(!roots.is_empty());
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers rejecting symlinked repository databases before SQLite opens them.
    fn ensure_regular_repository_database_rejects_symlink() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(test::setup_with_new_libra_in(repo.path()));
        let _guard = test::ChangeDirGuard::new(repo.path());
        let database = path::database();
        let external = repo.path().join("external.db");
        fs::remove_file(&database).unwrap();
        fs::write(&external, b"sqlite").unwrap();
        symlink(&external, &database).unwrap();

        let error = ensure_regular_repository_database().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("symlink repository database"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers rebase state commits being protected as roots.
    async fn collect_roots_includes_rebase_state() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let onto = test_hash(20);
        let orig_head = test_hash(21);
        let current_head = test_hash(22);
        let todo = test_hash(23);
        let done = test_hash(24);
        let stopped = test_hash(25);
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS rebase_state (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                head_name TEXT NOT NULL,
                onto TEXT NOT NULL,
                orig_head TEXT NOT NULL,
                current_head TEXT NOT NULL,
                todo TEXT NOT NULL,
                done TEXT NOT NULL,
                stopped_sha TEXT
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DELETE FROM rebase_state".to_string(),
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO rebase_state
                (head_name, onto, orig_head, current_head, todo, done, stopped_sha)
             VALUES ('main', ?, ?, ?, ?, ?, ?)",
            [
                onto.to_string().into(),
                orig_head.to_string().into(),
                current_head.to_string().into(),
                todo.to_string().into(),
                done.to_string().into(),
                stopped.to_string().into(),
            ],
        ))
        .await
        .unwrap();

        let (roots, _protected) = collect_roots_from_database().await.unwrap();

        for hash in [onto, orig_head, current_head, todo, done, stopped] {
            assert!(roots.contains(&hash));
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers legacy rebase-merge state being protected without migration.
    async fn rebase_state_roots_include_legacy_rebase_merge_files() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DROP TABLE IF EXISTS rebase_state".to_string(),
        ))
        .await
        .unwrap();
        let onto = test_hash(26);
        let orig_head = test_hash(27);
        let current_head = test_hash(28);
        let todo = test_hash(29);
        let done = test_hash(36);
        let stopped = test_hash(37);
        let legacy_dir = util::storage_path().join("rebase-merge");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(legacy_dir.join("onto"), onto.to_string()).unwrap();
        fs::write(legacy_dir.join("orig-head"), orig_head.to_string()).unwrap();
        fs::write(legacy_dir.join("current-head"), current_head.to_string()).unwrap();
        fs::write(legacy_dir.join("todo"), format!("{todo}\n")).unwrap();
        fs::write(legacy_dir.join("done"), format!("{done}\n")).unwrap();
        fs::write(legacy_dir.join("stopped-sha"), stopped.to_string()).unwrap();

        let roots = rebase_state_roots(&db).await.unwrap();

        for hash in [onto, orig_head, current_head, todo, done, stopped] {
            assert!(roots.contains(&hash));
        }
        assert!(legacy_dir.exists());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers legacy rebase roots being unioned with database state rows.
    async fn rebase_state_roots_union_database_and_legacy_state() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let db_root = test_hash(46);
        let legacy_root = test_hash(47);
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS rebase_state (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                head_name TEXT NOT NULL,
                onto TEXT NOT NULL,
                orig_head TEXT NOT NULL,
                current_head TEXT NOT NULL,
                todo TEXT NOT NULL,
                done TEXT NOT NULL,
                stopped_sha TEXT
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DELETE FROM rebase_state".to_string(),
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO rebase_state
                (head_name, onto, orig_head, current_head, todo, done, stopped_sha)
             VALUES ('main', ?, ?, ?, '', '', NULL)",
            [
                db_root.to_string().into(),
                test_hash(48).to_string().into(),
                test_hash(49).to_string().into(),
            ],
        ))
        .await
        .unwrap();
        let legacy_dir = util::storage_path().join("rebase-merge");
        fs::create_dir_all(&legacy_dir).unwrap();
        fs::write(legacy_dir.join("onto"), legacy_root.to_string()).unwrap();
        fs::write(legacy_dir.join("orig-head"), test_hash(60).to_string()).unwrap();
        fs::write(legacy_dir.join("current-head"), test_hash(61).to_string()).unwrap();

        let roots = rebase_state_roots(&db).await.unwrap();

        assert!(roots.contains(&db_root));
        assert!(roots.contains(&legacy_root));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers fail-closed decoding for malformed rebase state roots.
    async fn rebase_state_roots_rejects_malformed_columns() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DROP TABLE IF EXISTS rebase_state".to_string(),
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE rebase_state (
                onto TEXT,
                orig_head TEXT,
                current_head TEXT,
                todo TEXT,
                done TEXT,
                stopped_sha TEXT
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO rebase_state
                (onto, orig_head, current_head, todo, done, stopped_sha)
             VALUES (?, ?, NULL, '', '', NULL)",
            [
                test_hash(38).to_string().into(),
                test_hash(39).to_string().into(),
            ],
        ))
        .await
        .unwrap();

        let error = rebase_state_roots(&db).await.unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("rebase_state.current_head"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers bisect state commits being protected as roots.
    async fn collect_roots_includes_bisect_state() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        let orig_head = test_hash(30);
        let bad = test_hash(31);
        let good = test_hash(32);
        let current = test_hash(33);
        let skipped = test_hash(34);
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE bisect_state (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                orig_head TEXT NOT NULL,
                orig_head_name TEXT,
                bad TEXT,
                good TEXT NOT NULL,
                current TEXT,
                skipped TEXT,
                steps INTEGER,
                completed INTEGER NOT NULL DEFAULT 0
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO bisect_state
                (orig_head, orig_head_name, bad, good, current, skipped, steps, completed)
             VALUES (?, 'main', ?, ?, ?, ?, 1, 0)",
            [
                orig_head.to_string().into(),
                bad.to_string().into(),
                serde_json::json!([good.to_string()]).to_string().into(),
                current.to_string().into(),
                serde_json::json!([skipped.to_string()]).to_string().into(),
            ],
        ))
        .await
        .unwrap();

        let (roots, _protected) = collect_roots_from_database().await.unwrap();

        for hash in [orig_head, bad, good, current, skipped] {
            assert!(roots.contains(&hash));
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers fail-closed decoding for malformed bisect state roots.
    async fn bisect_state_roots_rejects_malformed_columns() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DROP TABLE IF EXISTS bisect_state".to_string(),
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE bisect_state (
                orig_head TEXT,
                bad TEXT,
                good TEXT,
                current TEXT,
                skipped TEXT
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO bisect_state
                (orig_head, bad, good, current, skipped)
             VALUES (?, NULL, NULL, NULL, NULL)",
            [test_hash(35).to_string().into()],
        ))
        .await
        .unwrap();

        let error = bisect_state_roots(&db).await.unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("bisect_state.good"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers merge state commits being protected as roots.
    async fn collect_roots_includes_merge_state_file() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let orig_head = test_hash(40);
        let target = test_hash(41);
        let base = test_hash(42);
        fs::write(
            util::storage_path().join("merge-state.json"),
            serde_json::json!({
                "head_name": "main",
                "orig_head": orig_head.to_string(),
                "target": target.to_string(),
                "target_ref": "topic",
                "base": base.to_string(),
                "conflicted_paths": []
            })
            .to_string(),
        )
        .unwrap();

        let (roots, _protected) = collect_roots_from_database().await.unwrap();

        assert!(roots.contains(&orig_head));
        assert!(roots.contains(&target));
        assert!(roots.contains(&base));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers older merge state files with missing optional object fields.
    async fn merge_state_roots_tolerates_missing_fields() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let orig_head = test_hash(43);
        fs::write(
            util::storage_path().join("merge-state.json"),
            serde_json::json!({
                "head_name": "main",
                "orig_head": orig_head.to_string(),
                "conflicted_paths": []
            })
            .to_string(),
        )
        .unwrap();

        let roots = merge_state_roots().unwrap();

        assert!(roots.contains(&orig_head));
        assert_eq!(roots.len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers present merge-state fields rejecting empty object IDs.
    async fn merge_state_roots_rejects_empty_present_fields() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        fs::write(
            util::storage_path().join("merge-state.json"),
            serde_json::json!({
                "head_name": "main",
                "orig_head": test_hash(44).to_string(),
                "target": "",
                "base": test_hash(45).to_string(),
                "conflicted_paths": []
            })
            .to_string(),
        )
        .unwrap();

        let error = merge_state_roots().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("merge-state.target"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers local-only object-index rows not pinning ordinary unreachable garbage.
    async fn object_index_protections_skip_local_only_repositories() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let _storage_type = test::ScopedEnvVar::set("LIBRA_STORAGE_TYPE", "");
        let _d1_account = test::ScopedEnvVar::set("LIBRA_D1_ACCOUNT_ID", "");
        let _d1_token = test::ScopedEnvVar::set("LIBRA_D1_API_TOKEN", "");
        let _d1_database = test::ScopedEnvVar::set("LIBRA_D1_DATABASE_ID", "");
        let _endpoint = test::ScopedEnvVar::set("LIBRA_STORAGE_ENDPOINT", "");
        let _access = test::ScopedEnvVar::set("LIBRA_STORAGE_ACCESS_KEY", "");
        let _secret = test::ScopedEnvVar::set("LIBRA_STORAGE_SECRET_KEY", "");
        ConfigKv::set("libra.repoid", "repo-a", false)
            .await
            .unwrap();
        let db = get_db_conn_instance().await;
        let unsynced = test_hash(57);
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO object_index (o_id, o_type, o_size, repo_id, created_at, is_synced) \
             VALUES (?, 'blob', 1, 'repo-a', 1, 0)",
            [unsynced.to_string().into()],
        ))
        .await
        .unwrap();

        let protected = object_index_prune_protections(&db).await.unwrap();

        assert!(protected.is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers cloud-pending object-index rows and agent checkpoints.
    async fn collect_roots_includes_cloud_and_agent_catalogs() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        ConfigKv::set("libra.repoid", "repo-a", false)
            .await
            .unwrap();
        let db = get_db_conn_instance().await;
        let unsynced = test_hash(50);
        let other_repo = test_hash(51);
        let synced = test_hash(52);
        let parent = test_hash(53);
        let tree = test_hash(54);
        let metadata = test_hash(55);
        let traces = test_hash(56);
        for (hash, repo_id, is_synced) in [
            (unsynced, "repo-a", 0),
            (other_repo, "repo-b", 0),
            (synced, "repo-a", 1),
        ] {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Sqlite,
                "INSERT INTO object_index (o_id, o_type, o_size, repo_id, created_at, is_synced) \
                 VALUES (?, 'blob', 1, ?, 1, ?)",
                [
                    hash.to_string().into(),
                    repo_id.into(),
                    i64::from(is_synced).into(),
                ],
            ))
            .await
            .unwrap();
        }
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS agent_session (
                session_id TEXT PRIMARY KEY,
                agent_kind TEXT NOT NULL,
                provider_session_id TEXT NOT NULL,
                state TEXT NOT NULL,
                working_dir TEXT NOT NULL,
                metadata_json TEXT NOT NULL,
                redaction_report TEXT NOT NULL,
                started_at INTEGER NOT NULL,
                last_event_at INTEGER NOT NULL,
                schema_version INTEGER NOT NULL
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT OR IGNORE INTO agent_session (
                session_id, agent_kind, provider_session_id, state, working_dir,
                metadata_json, redaction_report, started_at, last_event_at, schema_version
            ) VALUES ('session', 'codex', 'provider', 'active', '.', '{}', '{}', 1, 1, 1)",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS agent_checkpoint (
                checkpoint_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                parent_checkpoint_id TEXT,
                scope TEXT NOT NULL,
                parent_commit TEXT,
                tree_oid TEXT NOT NULL,
                metadata_blob_oid TEXT NOT NULL,
                traces_commit TEXT NOT NULL,
                tool_use_id TEXT,
                subagent_session_id TEXT,
                description TEXT,
                created_at INTEGER NOT NULL
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO agent_checkpoint
                (checkpoint_id, session_id, scope, parent_commit, tree_oid,
                 metadata_blob_oid, traces_commit, created_at)
             VALUES ('cp', 'session', 'temporary', ?, ?, ?, ?, 1)",
            [
                parent.to_string().into(),
                tree.to_string().into(),
                metadata.to_string().into(),
                traces.to_string().into(),
            ],
        ))
        .await
        .unwrap();

        ConfigKv::set("vault.env.LIBRA_STORAGE_TYPE", "r2", false)
            .await
            .unwrap();
        let (roots, protected) = collect_roots_from_database().await.unwrap();

        assert!(!roots.contains(&unsynced));
        assert!(protected.contains(&unsynced));
        assert!(!protected.contains(&other_repo));
        assert!(!protected.contains(&synced));
        for hash in [parent, tree, metadata, traces] {
            assert!(roots.contains(&hash));
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers unmerged index stages being protected as reachability roots.
    async fn index_roots_include_unmerged_stages() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let mut index = git_internal::internal::index::Index::new();
        let mut expected = HashSet::new();

        for stage in 1..=3 {
            let hash = test_hash(stage);
            let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
                "conflict.txt".to_string(),
                hash,
                stage as u32,
            );
            entry.flags.stage = stage;
            index.add(entry);
            expected.insert(hash);
        }
        index.to_file(path::index()).unwrap();

        let roots = index_roots().unwrap();
        for hash in expected {
            assert!(roots.contains(&hash));
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers fail-closed decoding for malformed agent checkpoint roots.
    async fn agent_checkpoint_roots_rejects_malformed_columns() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let db = get_db_conn_instance().await;
        db.execute(Statement::from_string(
            DbBackend::Sqlite,
            "DROP TABLE IF EXISTS agent_checkpoint".to_string(),
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "CREATE TABLE agent_checkpoint (
                parent_commit TEXT,
                tree_oid TEXT,
                metadata_blob_oid TEXT,
                traces_commit TEXT
            )",
            [],
        ))
        .await
        .unwrap();
        db.execute(Statement::from_sql_and_values(
            DbBackend::Sqlite,
            "INSERT INTO agent_checkpoint
                (parent_commit, tree_oid, metadata_blob_oid, traces_commit)
             VALUES (NULL, NULL, ?, ?)",
            [
                test_hash(58).to_string().into(),
                test_hash(59).to_string().into(),
            ],
        ))
        .await
        .unwrap();

        let error = agent_checkpoint_roots(&db).await.unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("agent_checkpoint.tree_oid"));
    }

    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    /// Covers rejecting symlinked indexes before loading index contents.
    fn index_roots_rejects_symlink_index() {
        use std::os::unix::fs::symlink;

        let repo = tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(test::setup_with_new_libra_in(repo.path()));
        let _guard = test::ChangeDirGuard::new(repo.path());
        let index = path::index();
        let external = repo.path().join("external.index");
        let _ = fs::remove_file(&index);
        fs::write(&external, b"index").unwrap();
        symlink(&external, &index).unwrap();

        let error = index_roots().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("symlink index"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers index roots ignoring gitlink entries.
    async fn index_roots_skip_gitlink_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let gitlink = test_hash(7);
        let mut index = git_internal::internal::index::Index::new();
        let mut entry = git_internal::internal::index::IndexEntry::new_from_blob(
            "submodule".to_string(),
            gitlink,
            0,
        );
        entry.mode = GITLINK_INDEX_MODE;
        index.add(entry);
        index.to_file(path::index()).unwrap();

        let roots = index_roots().unwrap();
        assert!(!roots.contains(&gitlink));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers file-backed stash references being used as reachability roots.
    async fn run_gc_preserves_file_backed_stash_ref() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let blob = Blob::from_content("stashed");
        save_test_object(&blob, &blob.id).unwrap();
        let stash_ref = util::storage_path().join("refs/stash");
        fs::create_dir_all(stash_ref.parent().unwrap()).unwrap();
        fs::write(&stash_ref, format!("{}\n", blob.id)).unwrap();

        let result = run_gc(&GcArgs {
            dry_run: false,
            prune: "now".to_string(),
            no_prune: false,
            aggressive: false,
            auto: false,
            force: false,
        })
        .await
        .unwrap();

        assert_eq!(result.loose_objects.pruned, 0);
        assert!(storage.exist(&blob.id));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers file-backed stash reflogs being used as reachability roots.
    async fn stash_roots_include_file_backed_reflog_entries() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let old_hash = test_hash(10);
        let new_hash = test_hash(11);
        let stash_log = util::storage_path().join("logs/refs/stash");
        fs::create_dir_all(stash_log.parent().unwrap()).unwrap();
        fs::write(
            &stash_log,
            format!("{old_hash} {new_hash} tester <tester@example.com> 1 +0000\tstash\n"),
        )
        .unwrap();

        let roots = stash_roots().unwrap();

        assert!(roots.contains(&old_hash));
        assert!(roots.contains(&new_hash));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers empty file-backed stash references failing closed.
    async fn stash_roots_rejects_empty_file_backed_ref() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let stash_ref = util::storage_path().join("refs/stash");
        fs::create_dir_all(stash_ref.parent().unwrap()).unwrap();
        fs::write(&stash_ref, " \n").unwrap();

        let error = stash_roots().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("stash reference"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers malformed stash reflog entries failing closed.
    async fn stash_roots_rejects_reflog_without_new_oid() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let stash_log = util::storage_path().join("logs/refs/stash");
        fs::create_dir_all(stash_log.parent().unwrap()).unwrap();
        fs::write(&stash_log, format!("{}\n", test_hash(12))).unwrap();

        let error = stash_roots().unwrap_err();

        assert_eq!(error.stable_code(), StableErrorCode::RepoCorrupt);
        assert!(error.render().contains("missing stash commit hash"));
    }

    #[tokio::test]
    #[serial_test::serial]
    /// Covers invalid prune-date errors through `execute_safe`.
    async fn execute_safe_rejects_invalid_prune() {
        let repo = tempdir().unwrap();
        test::setup_with_new_libra_in(repo.path()).await;
        let _guard = test::ChangeDirGuard::new(repo.path());
        let error = execute_safe(
            GcArgs {
                dry_run: false,
                prune: "bad-date".to_string(),
                no_prune: false,
                aggressive: false,
                auto: false,
                force: false,
            },
            &OutputConfig::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::CliInvalidArguments);
    }

    #[test]
    /// Covers grouping miscellaneous pack sidecar files.
    fn collect_pack_groups_groups_other_sidecars() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pack-abcd.tmp"), b"tmp").unwrap();
        let groups = collect_pack_groups(dir.path()).unwrap();
        assert_eq!(groups["pack-abcd"].others.len(), 1);
    }

    #[test]
    /// Covers dry-run reporting for stale pack files.
    fn handle_pack_file_reports_would_prune_in_dry_run() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pack-abcd.idx");
        fs::write(&path, b"idx").unwrap();
        let action = handle_pack_file(
            &path,
            PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1)),
            true,
            false,
            "orphan",
        )
        .unwrap();
        assert_eq!(action.action, PackAction::WouldPrune);
        assert!(path.exists());
    }

    #[test]
    /// Covers retaining stale pack files when pruning is disabled.
    fn handle_pack_file_retains_when_prune_disabled() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("pack-abcd.idx");
        fs::write(&path, b"idx").unwrap();
        let action = handle_pack_file(&path, PrunePolicy::Never, false, false, "orphan").unwrap();
        assert_eq!(action.action, PackAction::Retained);
        assert!(path.exists());
    }

    #[test]
    /// Covers retaining non-empty loose-object prefix directories.
    fn remove_empty_parent_dir_ignores_non_empty_directory() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("aa").join("object");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file.parent().unwrap().join("other"), b"x").unwrap();
        remove_empty_parent_dir(&file).unwrap();
        assert!(file.parent().unwrap().exists());
    }

    #[test]
    /// Covers stable JSON names for loose-object actions.
    fn gc_action_serialization_names_are_stable() {
        assert_eq!(
            serde_json::to_value(GcAction::WouldPrune).unwrap(),
            serde_json::json!("would_prune")
        );
    }

    #[test]
    /// Covers stable JSON names for pack-file actions.
    fn pack_action_serialization_names_are_stable() {
        assert_eq!(
            serde_json::to_value(PackAction::WouldPrune).unwrap(),
            serde_json::json!("would_prune")
        );
    }

    #[test]
    /// Covers the default prune constant used by help and docs.
    fn default_prune_constant_matches_help_contract() {
        assert_eq!(DEFAULT_PRUNE, "2.weeks.ago");
    }

    #[test]
    /// Covers the week duration constant.
    fn seconds_per_week_matches_days() {
        assert_eq!(SECONDS_PER_WEEK, 7 * SECONDS_PER_DAY);
    }

    #[test]
    /// Covers month and year duration approximations.
    fn longer_prune_constants_match_calendar_approximations() {
        assert_eq!(SECONDS_PER_MONTH, 30 * SECONDS_PER_DAY);
        assert_eq!(SECONDS_PER_YEAR, 365 * SECONDS_PER_DAY);
    }

    #[test]
    /// Covers examples mentioning dry-run and JSON modes.
    fn gc_examples_mentions_dry_run_and_json() {
        assert!(GC_EXAMPLES.contains("--dry-run"));
        assert!(GC_EXAMPLES.contains("--json"));
    }

    #[test]
    /// Covers retention reasons in pack-file actions.
    fn pack_file_action_can_report_retention_reason() {
        let action = PackFileAction {
            path: "pack-x.idx".into(),
            action: PackAction::Retained,
            reason: "kept".into(),
        };
        assert_eq!(action.reason, "kept");
    }

    #[test]
    /// Covers object action metadata fields.
    fn gc_object_action_can_report_prune_reason() {
        let action = GcObjectAction {
            oid: "abc".into(),
            object_type: "blob".into(),
            action: GcAction::Retained,
            reason: "young".into(),
        };
        assert_eq!(action.object_type, "blob");
    }

    #[test]
    /// Covers the default reachability accumulator.
    fn reachability_default_has_no_roots() {
        let reachability = Reachability::default();
        assert!(reachability.roots.is_empty());
        assert!(reachability.reachable.is_empty());
    }

    #[test]
    /// Covers the default pack group accumulator.
    fn pack_group_default_is_empty() {
        let group = PackGroup::default();
        assert!(group.pack.is_none());
        assert!(group.idx.is_none());
        assert!(group.keep.is_none());
    }

    #[test]
    /// Covers `PrunePolicy::Never`.
    fn should_prune_returns_false_for_never() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"x").unwrap();
        assert!(!should_prune(&path, PrunePolicy::Never).unwrap());
    }

    #[test]
    /// Covers pruning with a future cutoff.
    fn should_prune_accepts_future_cutoff() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("file");
        fs::write(&path, b"x").unwrap();
        assert!(
            should_prune(
                &path,
                PrunePolicy::OlderThan(SystemTime::now() + Duration::from_secs(1))
            )
            .unwrap()
        );
    }

    #[test]
    /// Covers missing-file metadata errors during prune checks.
    fn should_prune_reports_missing_file_metadata_error() {
        let dir = tempdir().unwrap();
        let error = should_prune(
            &dir.path().join("missing"),
            PrunePolicy::OlderThan(SystemTime::now()),
        )
        .unwrap_err();
        assert_eq!(error.stable_code(), StableErrorCode::IoReadFailed);
    }

    #[test]
    /// Covers blob objects having no graph children.
    fn object_children_blob_has_no_children() {
        let repo = tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(test::setup_with_new_libra_in(repo.path()));
        let _guard = test::ChangeDirGuard::new(repo.path());
        let storage = ClientStorage::local(path::objects());
        let blob = Blob::from_content("blob");
        save_test_object(&blob, &blob.id).unwrap();
        assert!(object_children(&storage, &blob.id).unwrap().is_empty());
    }
}
