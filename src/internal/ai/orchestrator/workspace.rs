//! Task workspace preparation and synchronization for orchestrated AI execution.
//!
//! Boundary: each task receives an isolated copy or FUSE overlay of the main workspace,
//! then allowed changes are synced back after scope checks. Tests in this module cover
//! file copy, symlink handling, deletion, contract violations, and cleanup behavior.

#[cfg(target_os = "macos")]
use std::sync::Mutex;
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
#[cfg(unix)]
use std::{
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use libfuse_fs::{
    overlayfs::{OverlayFs, config::Config as FuseOverlayConfig},
    passthrough::{PassthroughArgs, new_passthroughfs_layer},
};
#[cfg(unix)]
use rfuse3::{MountOptions, raw::Session};
#[cfg(unix)]
use tokio::runtime::Handle;
#[cfg(unix)]
use tracing::warn;
use uuid::Uuid;

use super::{
    acl::{ScopeVerdict, cargo_lock_companion_allowed, check_scope},
    types::TaskWorkspaceBackend,
};
use crate::{
    internal::ai::workspace_snapshot::{
        WorkspaceEntry, WorkspaceSnapshot, changed_paths_since_baseline, snapshot_workspace,
        snapshot_workspace_with_contents, workspace_entry_if_exists,
    },
    utils::util,
};

pub(crate) struct TaskWorktree {
    pub(crate) root: PathBuf,
    pub(crate) baseline: WorkspaceSnapshot,
    backend: TaskWorktreeBackend,
}

impl TaskWorktree {
    pub(crate) fn backend(&self) -> TaskWorkspaceBackend {
        match &self.backend {
            TaskWorktreeBackend::Copy { .. } => TaskWorkspaceBackend::Copy,
            #[cfg(unix)]
            TaskWorktreeBackend::Fuse(_) => TaskWorkspaceBackend::Fuse,
        }
    }
}

enum TaskWorktreeBackend {
    Copy {
        cleanup_root: PathBuf,
    },
    #[cfg(unix)]
    Fuse(FuseTaskWorktreeBackend),
}

#[cfg(unix)]
struct FuseTaskWorktreeBackend {
    cleanup_root: PathBuf,
    mount_handle: rfuse3::raw::MountHandle,
}

struct TaskWorktreePaths {
    cleanup_root: PathBuf,
    workspace_root: PathBuf,
    lower_root: PathBuf,
    upper_root: PathBuf,
}

#[cfg(target_os = "macos")]
static MACOS_FUSE_MOUNT_HANDSHAKE_LOCK: Mutex<()> = Mutex::new(());

#[cfg(all(unix, test))]
const FUSE_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_millis(10);
#[cfg(all(unix, not(test)))]
const FUSE_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(all(unix, test))]
const FUSE_HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(1);
#[cfg(all(unix, not(test)))]
const FUSE_HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(50);

/// FUSE provisioning gate. Once `disabled` flips to `true`,
/// `prepare_task_worktree` skips FUSE entirely and goes directly to the copy
/// backend.
///
/// The flag is shared via a single `Arc<AtomicBool>` so concurrent task
/// provisioning sees consistent state. Tasks that race into the *first*
/// FUSE attempt still snapshot and materialize their lower/upper directories
/// independently; whichever attempts finish first set the flag, and every
/// subsequent task short-circuits past the FUSE path. On macOS only the
/// `mount_macfuse` handshake is serialized after materialization, avoiding
/// device allocation races without delaying baseline capture.
///
/// The orchestrator owns one `FuseProvisionState` for its entire lifetime so
/// the disable signal persists across orchestrator runs in the same process —
/// not just across replans within a single run. Without this persistence the
/// orchestrator would re-attempt (and re-fail) FUSE on every new intent the
/// user submits in the same TUI session.
#[derive(Clone, Debug)]
pub struct FuseProvisionState {
    disabled: Arc<AtomicBool>,
}

impl Default for FuseProvisionState {
    fn default() -> Self {
        Self {
            disabled: Arc::new(AtomicBool::new(fuse_disabled_by_default())),
        }
    }
}

impl FuseProvisionState {
    /// Atomically mark FUSE disabled for this session. Returns `true` iff this
    /// call was the first to flip the flag; the caller is then responsible for
    /// emitting the one-time TUI note.
    pub fn disable_first_time(&self) -> bool {
        self.disabled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }
}

fn fuse_disabled_by_default() -> bool {
    #[cfg(test)]
    {
        true
    }
    #[cfg(not(test))]
    {
        std::env::var_os(crate::utils::pager::LIBRA_TEST_ENV).is_some()
    }
}

/// Outcome of a FUSE provisioning attempt during `prepare_task_worktree`.
/// Reported back to the caller so the orchestrator can emit a single
/// user-visible note when FUSE flips from "available" to "disabled".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FuseAttemptOutcome {
    /// FUSE overlay mounted successfully and is the active backend.
    Mounted,
    /// FUSE was already disabled session-wide before this task; copy backend used.
    Skipped,
    /// FUSE was already disabled by an earlier failure; copy backend used.
    AlreadyDisabled,
    /// This task was the first to fail FUSE — it triggered the session disable.
    /// The caller must emit the one-time "FUSE disabled" TUI note.
    JustDisabled { reason: String },
    /// Platform without FUSE support (non-unix); copy backend used.
    Unsupported,
}

impl FuseAttemptOutcome {
    pub fn disabled_reason(&self) -> Option<&str> {
        match self {
            Self::JustDisabled { reason } => Some(reason.as_str()),
            _ => None,
        }
    }
}

#[cfg(unix)]
enum FuseTaskWorktreeProvision {
    Mounted(TaskWorktreeBackend),
    Fallback { reason: String },
}

pub(crate) fn prepare_task_worktree(
    main_working_dir: &Path,
    task_id: Uuid,
    fuse_state: &FuseProvisionState,
) -> io::Result<(TaskWorktree, FuseAttemptOutcome)> {
    let baseline = snapshot_workspace_with_contents(main_working_dir)?;

    #[cfg(unix)]
    {
        if !fuse_state.is_disabled() {
            let fuse_paths = task_worktree_paths(task_id, "fuse");
            match prepare_fuse_task_worktree(main_working_dir, &fuse_paths, &baseline)? {
                FuseTaskWorktreeProvision::Mounted(backend) => {
                    return Ok((
                        TaskWorktree {
                            root: fuse_paths.workspace_root,
                            baseline,
                            backend,
                        },
                        FuseAttemptOutcome::Mounted,
                    ));
                }
                FuseTaskWorktreeProvision::Fallback { reason } => {
                    // Mount or health check failed; flip the session-wide flag.
                    let outcome = if fuse_state.disable_first_time() {
                        FuseAttemptOutcome::JustDisabled { reason }
                    } else {
                        FuseAttemptOutcome::AlreadyDisabled
                    };
                    return prepare_task_worktree_copy_fallback(
                        main_working_dir,
                        task_id,
                        baseline,
                        outcome,
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    let outcome = FuseAttemptOutcome::AlreadyDisabled;
    #[cfg(not(unix))]
    let outcome = FuseAttemptOutcome::Unsupported;

    prepare_task_worktree_copy_fallback(main_working_dir, task_id, baseline, outcome)
}

fn prepare_task_worktree_copy_fallback(
    main_working_dir: &Path,
    task_id: Uuid,
    baseline: WorkspaceSnapshot,
    outcome: FuseAttemptOutcome,
) -> io::Result<(TaskWorktree, FuseAttemptOutcome)> {
    let copy_paths = task_worktree_paths(task_id, "copy");
    prepare_task_worktree_root(&copy_paths.cleanup_root)?;
    let backend = prepare_copy_task_worktree(main_working_dir, &copy_paths, &baseline)?;

    Ok((
        TaskWorktree {
            root: copy_paths.workspace_root,
            baseline,
            backend,
        },
        outcome,
    ))
}

fn task_worktree_paths(task_id: Uuid, backend: &str) -> TaskWorktreePaths {
    let cleanup_root = task_worktree_temp_dir().join(format!(
        "libra-task-worktree-{}-{}-{}",
        backend,
        std::process::id(),
        task_id
    ));
    TaskWorktreePaths {
        workspace_root: cleanup_root.join("workspace"),
        lower_root: cleanup_root.join("lower"),
        upper_root: cleanup_root.join("upper"),
        cleanup_root,
    }
}

fn task_worktree_temp_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    #[cfg(target_os = "macos")]
    {
        fs::canonicalize(&temp_dir).unwrap_or(temp_dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        temp_dir
    }
}

fn prepare_task_worktree_root(cleanup_root: &Path) -> io::Result<()> {
    if cleanup_root.exists() {
        fs::remove_dir_all(cleanup_root)?;
    }
    fs::create_dir_all(cleanup_root)
}

fn prepare_copy_task_worktree(
    main_working_dir: &Path,
    paths: &TaskWorktreePaths,
    baseline: &WorkspaceSnapshot,
) -> io::Result<TaskWorktreeBackend> {
    fs::create_dir_all(&paths.workspace_root)?;
    match util::try_get_storage_path(Some(main_working_dir.to_path_buf())) {
        Ok(storage) => link_repo_storage(
            &storage,
            &paths.workspace_root.join(util::ROOT_DIR),
            "copy task worktree",
        )?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    materialize_workspace(main_working_dir, &paths.workspace_root, baseline)?;
    Ok(TaskWorktreeBackend::Copy {
        cleanup_root: paths.cleanup_root.clone(),
    })
}

#[cfg(unix)]
fn prepare_fuse_task_worktree(
    main_working_dir: &Path,
    paths: &TaskWorktreePaths,
    baseline: &WorkspaceSnapshot,
) -> io::Result<FuseTaskWorktreeProvision> {
    let Ok(runtime) = Handle::try_current() else {
        return Ok(FuseTaskWorktreeProvision::Fallback {
            reason: "tokio runtime unavailable for FUSE provisioning".to_string(),
        });
    };

    prepare_task_worktree_root(&paths.cleanup_root)?;
    fs::create_dir_all(&paths.workspace_root)?;
    fs::create_dir_all(&paths.lower_root)?;
    fs::create_dir_all(&paths.upper_root)?;
    materialize_workspace(main_working_dir, &paths.lower_root, baseline)?;

    let expect_repo_storage_link =
        match util::try_get_storage_path(Some(main_working_dir.to_path_buf())) {
            Ok(storage) => {
                link_repo_storage(
                    &storage,
                    &paths.upper_root.join(util::ROOT_DIR),
                    "FUSE upper layer",
                )?;
                true
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => false,
            Err(err) => return Err(err),
        };

    let mount_result = mount_fuse_task_worktree_on_runtime(
        &runtime,
        &paths.lower_root,
        &paths.workspace_root,
        &paths.upper_root,
    );

    match mount_result {
        Ok(mount_handle) => {
            if let Err(err) =
                verify_fuse_task_worktree_mount(&paths.workspace_root, expect_repo_storage_link)
            {
                if let Err(unmount_err) = runtime.block_on(mount_handle.unmount()) {
                    warn!(
                        mount = %paths.workspace_root.display(),
                        "failed to unmount unhealthy FUSE task worktree before fallback: {}",
                        unmount_err
                    );
                }
                warn_cleanup_root_failure(&paths.cleanup_root);
                let reason = err.to_string();
                warn!(
                    path = %main_working_dir.display(),
                    mount = %paths.workspace_root.display(),
                    "mounted FUSE task worktree failed health check, falling back to copy backend: {}",
                    reason
                );
                return Ok(FuseTaskWorktreeProvision::Fallback { reason });
            }

            Ok(FuseTaskWorktreeProvision::Mounted(
                TaskWorktreeBackend::Fuse(FuseTaskWorktreeBackend {
                    cleanup_root: paths.cleanup_root.clone(),
                    mount_handle,
                }),
            ))
        }
        Err(err) => {
            warn_cleanup_root_failure(&paths.cleanup_root);
            let reason = err.to_string();
            warn!(
                path = %main_working_dir.display(),
                mount = %paths.workspace_root.display(),
                "failed to mount FUSE task worktree, falling back to copy backend: {}",
                reason
            );
            Ok(FuseTaskWorktreeProvision::Fallback { reason })
        }
    }
}

#[cfg(unix)]
fn mount_fuse_task_worktree_on_runtime(
    runtime: &Handle,
    lower_root: &Path,
    workspace_root: &Path,
    upper_root: &Path,
) -> io::Result<rfuse3::raw::MountHandle> {
    #[cfg(target_os = "macos")]
    {
        let _guard = MACOS_FUSE_MOUNT_HANDSHAKE_LOCK
            .lock()
            .map_err(|_| io::Error::other("macOS FUSE mount handshake lock poisoned"))?;
        runtime.block_on(mount_fuse_task_worktree(
            lower_root,
            workspace_root,
            upper_root,
        ))
    }

    #[cfg(not(target_os = "macos"))]
    {
        runtime.block_on(mount_fuse_task_worktree(
            lower_root,
            workspace_root,
            upper_root,
        ))
    }
}

#[cfg(unix)]
fn verify_fuse_task_worktree_mount(
    workspace_root: &Path,
    expect_repo_storage_link: bool,
) -> io::Result<()> {
    let started = Instant::now();
    let mut attempts = 0_u32;

    loop {
        attempts += 1;
        match verify_fuse_task_worktree_mount_once(workspace_root, expect_repo_storage_link) {
            Ok(()) => return Ok(()),
            Err(err) if started.elapsed() >= FUSE_HEALTH_CHECK_TIMEOUT => {
                return Err(io::Error::new(
                    err.kind(),
                    format!(
                        "FUSE mount health check failed after {} attempts over {:?}: workspace={}, expected_repo_storage_link={}: {}",
                        attempts,
                        started.elapsed(),
                        workspace_root.display(),
                        expect_repo_storage_link,
                        err
                    ),
                ));
            }
            Err(_) => thread::sleep(FUSE_HEALTH_CHECK_INTERVAL),
        }
    }
}

#[cfg(unix)]
fn verify_fuse_task_worktree_mount_once(
    workspace_root: &Path,
    expect_repo_storage_link: bool,
) -> io::Result<()> {
    fs::read_dir(workspace_root).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "mounted workspace root is not readable at '{}': {}",
                workspace_root.display(),
                err
            ),
        )
    })?;

    if expect_repo_storage_link {
        let storage_link = workspace_root.join(util::ROOT_DIR);
        fs::symlink_metadata(&storage_link).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "expected .libra repository storage link is not visible at '{}': {}",
                    storage_link.display(),
                    err
                ),
            )
        })?;
    }

    Ok(())
}

#[cfg(unix)]
async fn mount_fuse_task_worktree(
    lower_root: &Path,
    workspace_root: &Path,
    upper_root: &Path,
) -> io::Result<rfuse3::raw::MountHandle> {
    let lower_layer = Arc::new(
        new_passthroughfs_layer(PassthroughArgs {
            root_dir: lower_root,
            mapping: None::<&str>,
        })
        .await
        .map_err(|err| {
            fuse_mount_step_error(
                format!(
                    "failed to create FUSE lower passthrough layer at {}",
                    lower_root.display()
                ),
                err,
            )
        })?,
    );
    let upper_layer = Arc::new(
        new_passthroughfs_layer(PassthroughArgs {
            root_dir: upper_root,
            mapping: None::<&str>,
        })
        .await
        .map_err(|err| {
            fuse_mount_step_error(
                format!(
                    "failed to create FUSE upper passthrough layer at {}",
                    upper_root.display()
                ),
                err,
            )
        })?,
    );

    let overlay = OverlayFs::new(
        Some(upper_layer),
        vec![lower_layer],
        FuseOverlayConfig {
            mountpoint: workspace_root.to_path_buf(),
            do_import: true,
            ..Default::default()
        },
        1,
    )
    .map_err(|err| {
        fuse_mount_step_error(
            format!(
                "failed to create FUSE overlay for mount {}",
                workspace_root.display()
            ),
            err,
        )
    })?;

    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    let mut mount_options = MountOptions::default();
    #[cfg(target_os = "linux")]
    mount_options.force_readdir_plus(true);
    mount_options
        .uid(uid)
        .gid(gid)
        .fs_name("libra-task-worktree");

    Session::new(mount_options)
        .mount_with_unprivileged(overlay, workspace_root.as_os_str())
        .await
        .map_err(|err| {
            fuse_mount_step_error(
                format!(
                    "failed to mount FUSE overlay at {}",
                    workspace_root.display()
                ),
                err,
            )
        })
}

#[cfg(unix)]
fn fuse_mount_step_error(context: String, err: io::Error) -> io::Error {
    io::Error::new(err.kind(), format!("{context}: {err}"))
}

pub(crate) fn cleanup_task_worktree(worktree: TaskWorktree) -> io::Result<()> {
    match worktree.backend {
        TaskWorktreeBackend::Copy { cleanup_root } => remove_cleanup_root(&cleanup_root),
        #[cfg(unix)]
        TaskWorktreeBackend::Fuse(fuse) => cleanup_fuse_task_worktree(fuse),
    }
}

#[cfg(unix)]
fn cleanup_fuse_task_worktree(worktree: FuseTaskWorktreeBackend) -> io::Result<()> {
    let runtime = Handle::try_current().map_err(|err| {
        io::Error::other(format!("tokio runtime unavailable for FUSE cleanup: {err}"))
    })?;
    runtime.block_on(worktree.mount_handle.unmount())?;
    remove_cleanup_root(&worktree.cleanup_root)
}

fn remove_cleanup_root(cleanup_root: &Path) -> io::Result<()> {
    if cleanup_root.exists() {
        fs::remove_dir_all(cleanup_root)?;
    }
    Ok(())
}

#[cfg(unix)]
fn warn_cleanup_root_failure(cleanup_root: &Path) {
    if let Err(err) = remove_cleanup_root(cleanup_root) {
        warn!(
            path = %cleanup_root.display(),
            "failed to clean up abandoned task worktree root: {}",
            err
        );
    }
}

pub(crate) fn sync_task_worktree_back(
    main_working_dir: &Path,
    task_worktree_dir: &Path,
    baseline: &WorkspaceSnapshot,
    touch_files: &[String],
    in_scope: &[String],
    out_of_scope: &[String],
) -> Result<SyncBackReport, WorkspaceSyncError> {
    let task_snapshot = snapshot_workspace(task_worktree_dir)
        .map_err(|err| workspace_sync_io_error("snapshot task worktree", task_worktree_dir, err))?;
    let changed_paths = changed_paths_since_baseline(baseline, &task_snapshot);
    let changed_path_set = changed_paths.iter().cloned().collect::<BTreeSet<_>>();

    let violations =
        collect_contract_violations(&changed_paths, touch_files, in_scope, out_of_scope);
    if !violations.is_empty() {
        return Err(WorkspaceSyncError::ContractViolation(
            format_contract_violation_message(&violations),
        ));
    }

    let mut report = SyncBackReport::default();
    for rel_path in changed_paths {
        let baseline_entry = baseline.entries.get(&rel_path).cloned();
        let task_entry = task_snapshot.entries.get(&rel_path).cloned();
        let main_path = main_working_dir.join(&rel_path);
        let current_entry = workspace_entry_if_exists(&main_path)
            .map_err(|err| workspace_sync_io_error("inspect main workspace", &main_path, err))?;

        if current_entry == task_entry {
            report.already_applied.push(rel_path);
            continue;
        }

        if stale_cargo_lock_companion(&rel_path, &changed_path_set) {
            report.skipped.push(SkippedSyncPath {
                path: rel_path,
                reason: "Cargo.lock changed without a matching Cargo.toml change; treating it as a stale verification side effect".to_string(),
            });
            continue;
        }

        if current_entry == baseline_entry {
            apply_task_change(
                task_worktree_dir,
                main_working_dir,
                &rel_path,
                &task_snapshot,
            )?;
            report.applied.push(rel_path);
            continue;
        }

        if is_cargo_lock_path(&rel_path)
            && cargo_manifest_changed_for_lock(&rel_path, &changed_path_set)
        {
            return Err(WorkspaceSyncError::RetryableConflict {
                path: rel_path,
                reason: "Cargo.toml and Cargo.lock both changed, but the main workspace lockfile diverged from this task's lockfile".to_string(),
            });
        }

        if try_merge_text_change(
            main_working_dir,
            task_worktree_dir,
            baseline,
            &rel_path,
            baseline_entry.as_ref(),
            current_entry.as_ref(),
            task_entry.as_ref(),
        )? {
            report.merged.push(rel_path);
            continue;
        }

        return Err(WorkspaceSyncError::RetryableConflict {
            path: rel_path,
            reason:
                "main workspace changed concurrently and the task change could not be merged safely"
                    .to_string(),
        });
    }

    Ok(report)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SyncBackReport {
    pub(crate) applied: Vec<PathBuf>,
    pub(crate) already_applied: Vec<PathBuf>,
    pub(crate) merged: Vec<PathBuf>,
    pub(crate) skipped: Vec<SkippedSyncPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SkippedSyncPath {
    pub(crate) path: PathBuf,
    pub(crate) reason: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkspaceSyncError {
    #[error("{0}")]
    ContractViolation(String),
    #[error("retryable sync conflict at '{path}': {reason}")]
    RetryableConflict { path: PathBuf, reason: String },
    #[error("hard sync conflict: {reason}")]
    HardConflict {
        path: Option<PathBuf>,
        reason: String,
    },
    #[error("FUSE infrastructure failure while {stage} at '{path}': {message}")]
    FuseInfrastructure {
        stage: &'static str,
        path: PathBuf,
        message: String,
    },
    #[error("workspace sync failed while {stage} at '{path}': {source}")]
    Io {
        stage: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl WorkspaceSyncError {
    pub(crate) fn is_retryable_conflict(&self) -> bool {
        matches!(self, Self::RetryableConflict { .. })
    }

    pub(crate) fn is_fuse_infrastructure(&self) -> bool {
        matches!(self, Self::FuseInfrastructure { .. })
    }
}

fn apply_task_change(
    task_worktree_dir: &Path,
    main_working_dir: &Path,
    rel_path: &Path,
    task_snapshot: &WorkspaceSnapshot,
) -> Result<(), WorkspaceSyncError> {
    if task_snapshot.entries.contains_key(rel_path) {
        copy_workspace_entry(task_worktree_dir, main_working_dir, rel_path).map_err(|err| {
            workspace_sync_io_error("apply task change", &task_worktree_dir.join(rel_path), err)
        })?;
    } else {
        remove_workspace_entry(main_working_dir, rel_path).map_err(|err| {
            workspace_sync_io_error(
                "remove task-deleted path",
                &main_working_dir.join(rel_path),
                err,
            )
        })?;
    }
    Ok(())
}

fn stale_cargo_lock_companion(rel_path: &Path, changed_paths: &BTreeSet<PathBuf>) -> bool {
    is_cargo_lock_path(rel_path) && !cargo_manifest_changed_for_lock(rel_path, changed_paths)
}

fn is_cargo_lock_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "Cargo.lock")
}

fn cargo_manifest_changed_for_lock(lock_path: &Path, changed_paths: &BTreeSet<PathBuf>) -> bool {
    cargo_manifest_for_lock(lock_path)
        .as_ref()
        .is_some_and(|manifest| changed_paths.contains(manifest))
}

fn cargo_manifest_for_lock(lock_path: &Path) -> Option<PathBuf> {
    if !is_cargo_lock_path(lock_path) {
        return None;
    }
    let mut manifest = lock_path.to_path_buf();
    manifest.set_file_name("Cargo.toml");
    Some(manifest)
}

fn try_merge_text_change(
    main_working_dir: &Path,
    task_worktree_dir: &Path,
    baseline: &WorkspaceSnapshot,
    rel_path: &Path,
    baseline_entry: Option<&WorkspaceEntry>,
    current_entry: Option<&WorkspaceEntry>,
    task_entry: Option<&WorkspaceEntry>,
) -> Result<bool, WorkspaceSyncError> {
    if !matches!(
        (baseline_entry, current_entry, task_entry),
        (
            Some(WorkspaceEntry::File(_)),
            Some(WorkspaceEntry::File(_)),
            Some(WorkspaceEntry::File(_))
        )
    ) {
        return Ok(false);
    }

    let Some(baseline_bytes) = baseline.file_contents.get(rel_path) else {
        return Err(WorkspaceSyncError::HardConflict {
            path: Some(rel_path.to_path_buf()),
            reason: "baseline content is unavailable for three-way merge".to_string(),
        });
    };
    let main_path = main_working_dir.join(rel_path);
    let task_path = task_worktree_dir.join(rel_path);
    let current_bytes = fs::read(&main_path)
        .map_err(|err| workspace_sync_io_error("read main file for merge", &main_path, err))?;
    let task_bytes = fs::read(&task_path)
        .map_err(|err| workspace_sync_io_error("read task file for merge", &task_path, err))?;

    match diffy::merge_bytes(baseline_bytes, &current_bytes, &task_bytes) {
        Ok(merged) => {
            fs::write(&main_path, merged)
                .map_err(|err| workspace_sync_io_error("write merged file", &main_path, err))?;
            Ok(true)
        }
        Err(_) => Ok(false),
    }
}

fn workspace_sync_io_error(stage: &'static str, path: &Path, err: io::Error) -> WorkspaceSyncError {
    if is_fuse_infrastructure_io_error(&err) {
        return WorkspaceSyncError::FuseInfrastructure {
            stage,
            path: path.to_path_buf(),
            message: err.to_string(),
        };
    }

    WorkspaceSyncError::Io {
        stage,
        path: path.to_path_buf(),
        source: err,
    }
}

fn is_fuse_infrastructure_io_error(err: &io::Error) -> bool {
    err.raw_os_error() == Some(6) || is_fuse_infrastructure_error_message(&err.to_string())
}

pub(crate) fn is_fuse_infrastructure_error_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("device not configured") || lower.contains("os error 6")
}

/// Snapshot the worktree at `task_worktree_dir` and report every changed path
/// that violates the task contract relative to `baseline`.
///
/// Why: the executor needs to surface these violations to the LLM *inside* the
/// retry loop instead of letting them slip through to a terminal sync-back
/// failure that would force a full replan.
pub(crate) fn detect_contract_violations(
    task_worktree_dir: &Path,
    baseline: &WorkspaceSnapshot,
    touch_files: &[String],
    in_scope: &[String],
    out_of_scope: &[String],
) -> io::Result<Vec<ContractViolation>> {
    let task_snapshot = snapshot_workspace(task_worktree_dir)?;
    let changed_paths = changed_paths_since_baseline(baseline, &task_snapshot);
    Ok(collect_contract_violations(
        &changed_paths,
        touch_files,
        in_scope,
        out_of_scope,
    ))
}

#[derive(Clone, Debug)]
pub(crate) struct ContractViolation {
    pub(crate) path: PathBuf,
    pub(crate) reason: String,
}

pub(crate) fn format_contract_violation_message(violations: &[ContractViolation]) -> String {
    let mut parts = Vec::with_capacity(violations.len());
    for violation in violations {
        parts.push(format!(
            "task worktree modified '{}' outside its declared contract: {}",
            violation.path.display(),
            violation.reason
        ));
    }
    parts.join("\n")
}

fn collect_contract_violations(
    changed_paths: &[PathBuf],
    touch_files: &[String],
    in_scope: &[String],
    out_of_scope: &[String],
) -> Vec<ContractViolation> {
    changed_paths
        .iter()
        .filter_map(|rel_path| {
            let rel_path_str = rel_path.to_string_lossy();
            sync_contract_violation(touch_files, in_scope, out_of_scope, &rel_path_str).map(
                |reason| ContractViolation {
                    path: rel_path.clone(),
                    reason,
                },
            )
        })
        .collect()
}

fn sync_contract_violation(
    touch_files: &[String],
    in_scope: &[String],
    out_of_scope: &[String],
    path: &str,
) -> Option<String> {
    if !touch_files.is_empty() {
        if let ScopeVerdict::OutOfScope(reason) = check_scope(&[], out_of_scope, path) {
            return Some(reason);
        }
        if cargo_lock_companion_allowed(touch_files, path) {
            return None;
        }
        return match check_scope(touch_files, &[], path) {
            ScopeVerdict::InScope => None,
            ScopeVerdict::OutOfScope(reason) => Some(format!("not in touchFiles: {reason}")),
        };
    }

    if let ScopeVerdict::OutOfScope(reason) = check_scope(&[], out_of_scope, path) {
        return Some(reason);
    }
    if cargo_lock_companion_allowed(in_scope, path) {
        return None;
    }
    match check_scope(in_scope, out_of_scope, path) {
        ScopeVerdict::InScope => None,
        ScopeVerdict::OutOfScope(reason) => Some(reason),
    }
}

fn materialize_workspace(
    source_root: &Path,
    target_root: &Path,
    snapshot: &WorkspaceSnapshot,
) -> io::Result<()> {
    for rel_path in snapshot.entries.keys() {
        copy_workspace_entry(source_root, target_root, rel_path)?;
    }
    Ok(())
}

fn copy_workspace_entry(source_root: &Path, target_root: &Path, rel_path: &Path) -> io::Result<()> {
    let source = source_root.join(rel_path);
    let target = target_root.join(rel_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }

    let source_metadata = fs::symlink_metadata(&source)?;
    if source_metadata.file_type().is_symlink() {
        copy_symlink(&source, &target)?;
        return Ok(());
    }

    clone_or_copy_file(&source, &target)?;
    fs::set_permissions(&target, source_metadata.permissions())?;
    Ok(())
}

fn clone_or_copy_file(source: &Path, target: &Path) -> io::Result<()> {
    remove_existing_target(target)?;

    match try_clone_file_cow(source, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = remove_existing_target(target);
            fs::copy(source, target)?;
            Ok(())
        }
    }
}

fn copy_symlink(source: &Path, target: &Path) -> io::Result<()> {
    remove_existing_target(target)?;
    let link_target = fs::read_link(source)?;
    create_symlink(&link_target, source, target)
}

#[cfg(target_os = "macos")]
fn try_clone_file_cow(source: &Path, target: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    unsafe extern "C" {
        fn clonefile(
            src: *const libc::c_char,
            dst: *const libc::c_char,
            flags: libc::c_int,
        ) -> libc::c_int;
    }

    let source_cstr = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "source path contains interior NUL byte: {}",
                source.display()
            ),
        )
    })?;
    let target_cstr = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "target path contains interior NUL byte: {}",
                target.display()
            ),
        )
    })?;

    // SAFETY: The C strings are NUL-terminated, live for the duration of the call,
    // and `clonefile` does not retain the provided pointers after returning.
    let rc = unsafe { clonefile(source_cstr.as_ptr(), target_cstr.as_ptr(), 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn try_clone_file_cow(source: &Path, target: &Path) -> io::Result<()> {
    use std::{fs::File, os::fd::AsRawFd};

    const FICLONE: libc::c_ulong = 0x4004_9409;

    let source_file = File::open(source)?;
    let target_file = File::create(target)?;
    // SAFETY: `ioctl(FICLONE)` reads the source fd value, operates on two live
    // file descriptors opened above, and does not outlive the call boundary.
    let rc = unsafe { libc::ioctl(target_file.as_raw_fd(), FICLONE, source_file.as_raw_fd()) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn try_clone_file_cow(_source: &Path, _target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copy-on-write cloning is not supported on this platform",
    ))
}

fn remove_workspace_entry(root: &Path, rel_path: &Path) -> io::Result<()> {
    let target = root.join(rel_path);
    match fs::symlink_metadata(&target) {
        Ok(_) => {
            remove_existing_target(&target)?;
            remove_empty_parents(root, target.parent());
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

fn remove_existing_target(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn link_repo_storage(storage: &Path, link_path: &Path, context: &str) -> io::Result<()> {
    create_storage_link(storage, link_path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to link repository storage '{}' into {} at '{}': {}",
                storage.display(),
                context,
                link_path.display(),
                err
            ),
        )
    })
}

fn remove_empty_parents(root: &Path, mut current: Option<&Path>) {
    while let Some(dir) = current {
        if dir == root {
            break;
        }

        let is_empty = match fs::read_dir(dir) {
            Ok(mut entries) => entries.next().is_none(),
            Err(_) => false,
        };
        if !is_empty {
            break;
        }
        if fs::remove_dir(dir).is_err() {
            break;
        }
        current = dir.parent();
    }
}

#[cfg(unix)]
fn create_storage_link(storage: &Path, link_path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(storage, link_path)
}

#[cfg(unix)]
fn create_symlink(link_target: &Path, _source: &Path, link_path: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(link_target, link_path)
}

#[cfg(windows)]
fn create_storage_link(storage: &Path, link_path: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(storage, link_path)
}

#[cfg(windows)]
fn create_symlink(link_target: &Path, source: &Path, link_path: &Path) -> io::Result<()> {
    match fs::metadata(source) {
        Ok(metadata) if metadata.is_dir() => {
            std::os::windows::fs::symlink_dir(link_target, link_path)
        }
        _ => std::os::windows::fs::symlink_file(link_target, link_path),
    }
}

#[cfg(test)]
mod tests {
    use std::{io, path::PathBuf};

    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        FuseProvisionState, WorkspaceSyncError, cleanup_task_worktree, clone_or_copy_file,
        detect_contract_violations, materialize_workspace, prepare_copy_task_worktree,
        prepare_task_worktree, prepare_task_worktree_root, sync_task_worktree_back,
        task_worktree_paths,
    };
    use crate::{
        internal::ai::workspace_snapshot::{
            WorkspaceEntry, snapshot_workspace, snapshot_workspace_with_contents,
        },
        utils::{test, util},
    };

    #[cfg(unix)]
    fn symlink_path(target: &std::path::Path, link: &std::path::Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink_path(target: &std::path::Path, link: &std::path::Path) -> io::Result<()> {
        match std::fs::metadata(target) {
            Ok(metadata) if metadata.is_dir() => std::os::windows::fs::symlink_dir(target, link),
            _ => std::os::windows::fs::symlink_file(target, link),
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn fuse_passthrough_lookup_handles_symlink_entries_on_macos() {
        use std::ffi::OsStr;

        use libfuse_fs::passthrough::{PassthroughArgs, new_passthroughfs_layer};
        use rfuse3::raw::{Filesystem, Request};

        let root = tempdir().unwrap();
        std::fs::write(root.path().join("target.txt"), "target").unwrap();
        symlink_path(
            std::path::Path::new("target.txt"),
            &root.path().join("link.txt"),
        )
        .unwrap();

        let fs = new_passthroughfs_layer(PassthroughArgs {
            root_dir: root.path(),
            mapping: None::<&str>,
        })
        .await
        .unwrap();

        let entry = fs
            .lookup(Request::default(), 1, OsStr::new("link.txt"))
            .await
            .unwrap();

        assert_eq!(entry.attr.kind, rfuse3::FileType::Symlink);
    }

    #[test]
    fn clone_or_copy_file_preserves_contents() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source.txt");
        let target = temp.path().join("target.txt");
        std::fs::write(&source, "cow me maybe\n").unwrap();

        clone_or_copy_file(&source, &target).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "cow me maybe\n");
    }

    #[test]
    fn snapshot_records_directory_symlink_without_recursing() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        let external = temp.path().join("external");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("secret.txt"), "outside\n").unwrap();
        symlink_path(&external, &root.join("nested").join("external-link")).unwrap();

        let snapshot = snapshot_workspace(&root).unwrap();

        assert_eq!(
            snapshot
                .entries
                .get(std::path::Path::new("nested/external-link")),
            Some(&WorkspaceEntry::Symlink(external))
        );
        assert!(
            !snapshot
                .entries
                .contains_key(std::path::Path::new("nested/external-link/secret.txt"))
        );
    }

    #[test]
    fn materialize_and_sync_preserve_symlink_entries() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("target.txt"), "base\n").unwrap();
        symlink_path(std::path::Path::new("target.txt"), &main.join("link.txt")).unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        assert!(
            std::fs::symlink_metadata(task.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );

        std::fs::remove_file(task.join("link.txt")).unwrap();
        symlink_path(std::path::Path::new("updated.txt"), &task.join("link.txt")).unwrap();

        sync_task_worktree_back(&main, &task, &baseline, &[], &[], &[]).unwrap();

        assert!(
            std::fs::symlink_metadata(main.join("link.txt"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(main.join("link.txt")).unwrap(),
            PathBuf::from("updated.txt")
        );
    }

    #[test]
    fn sync_rejects_changes_outside_touch_files_contract() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/allowed.rs"), "base\n").unwrap();
        std::fs::write(main.join("src/other.rs"), "base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("src/other.rs"), "changed\n").unwrap();

        let err = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["src/allowed.rs".to_string()],
            &["src/".to_string()],
            &[],
        )
        .unwrap_err();

        assert!(err.to_string().contains("outside its declared contract"));
        assert_eq!(
            std::fs::read_to_string(main.join("src/other.rs")).unwrap(),
            "base\n"
        );
    }

    #[test]
    fn workspace_sync_skips_stale_cargo_lock_companion_and_ignores_target_outputs() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("libra/src")).unwrap();
        std::fs::write(
            main.join("libra/Cargo.toml"),
            "[package]\nname = \"libra\"\n",
        )
        .unwrap();
        std::fs::write(main.join("libra/src/main.rs"), "fn main() {}\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("libra/Cargo.lock"), "# generated lockfile\n").unwrap();
        std::fs::create_dir_all(task.join("libra/target")).unwrap();
        std::fs::write(task.join("libra/target/.rustc_info.json"), "{}\n").unwrap();

        let report = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &[
                "libra/Cargo.toml".to_string(),
                "libra/src/main.rs".to_string(),
            ],
            &["libra/".to_string()],
            &[],
        )
        .unwrap();

        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].path, PathBuf::from("libra/Cargo.lock"));
        assert!(!main.join("libra/Cargo.lock").exists());
        assert!(!main.join("libra/target/.rustc_info.json").exists());
    }

    #[test]
    fn workspace_sync_applies_cargo_lock_when_manifest_changed_from_baseline() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("libra")).unwrap();
        std::fs::write(
            main.join("libra/Cargo.toml"),
            "[package]\nname = \"libra\"\n\n[dependencies]\n",
        )
        .unwrap();
        std::fs::write(main.join("libra/Cargo.lock"), "# base lockfile\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(
            task.join("libra/Cargo.toml"),
            "[package]\nname = \"libra\"\n\n[dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        std::fs::write(task.join("libra/Cargo.lock"), "# updated lockfile\n").unwrap();

        let report = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["libra/Cargo.toml".to_string()],
            &["libra/".to_string()],
            &[],
        )
        .unwrap();

        assert!(report.applied.contains(&PathBuf::from("libra/Cargo.lock")));
        assert_eq!(
            std::fs::read_to_string(main.join("libra/Cargo.lock")).unwrap(),
            "# updated lockfile\n"
        );
    }

    #[test]
    fn workspace_sync_skips_already_applied_path() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("src/lib.rs"), "updated\n").unwrap();
        std::fs::write(main.join("src/lib.rs"), "updated\n").unwrap();

        let report = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["src/lib.rs".to_string()],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(report.already_applied, vec![PathBuf::from("src/lib.rs")]);
        assert!(report.applied.is_empty());
    }

    #[test]
    fn workspace_sync_three_way_merges_non_overlapping_text_edits() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();

        let baseline = snapshot_workspace_with_contents(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("src/lib.rs"), "ONE\ntwo\nthree\n").unwrap();
        std::fs::write(main.join("src/lib.rs"), "one\ntwo\nTHREE\n").unwrap();

        let report = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["src/lib.rs".to_string()],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(report.merged, vec![PathBuf::from("src/lib.rs")]);
        assert_eq!(
            std::fs::read_to_string(main.join("src/lib.rs")).unwrap(),
            "ONE\ntwo\nTHREE\n"
        );
    }

    #[test]
    fn workspace_sync_three_way_conflict_does_not_overwrite_main_workspace() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "one\ntwo\nthree\n").unwrap();

        let baseline = snapshot_workspace_with_contents(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("src/lib.rs"), "one\nTASK\nthree\n").unwrap();
        std::fs::write(main.join("src/lib.rs"), "one\nMAIN\nthree\n").unwrap();

        let err = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["src/lib.rs".to_string()],
            &[],
            &[],
        )
        .unwrap_err();

        assert!(matches!(err, WorkspaceSyncError::RetryableConflict { .. }));
        assert_eq!(
            std::fs::read_to_string(main.join("src/lib.rs")).unwrap(),
            "one\nMAIN\nthree\n"
        );
    }

    #[test]
    fn workspace_sync_manifest_and_lock_divergence_returns_retryable_conflict() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("Cargo.toml"), "[dependencies]\n").unwrap();
        std::fs::write(main.join("Cargo.lock"), "# base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("Cargo.toml"), "[dependencies]\nserde = \"1\"\n").unwrap();
        std::fs::write(task.join("Cargo.lock"), "# task lock\n").unwrap();
        std::fs::write(main.join("Cargo.lock"), "# concurrent lock\n").unwrap();

        let err = sync_task_worktree_back(
            &main,
            &task,
            &baseline,
            &["Cargo.toml".to_string()],
            &[],
            &[],
        )
        .unwrap_err();

        assert!(matches!(
            err,
            WorkspaceSyncError::RetryableConflict { path, .. }
                if path == std::path::Path::new("Cargo.lock")
        ));
        assert_eq!(
            std::fs::read_to_string(main.join("Cargo.lock")).unwrap(),
            "# concurrent lock\n"
        );
    }

    #[test]
    fn detect_contract_violations_reports_path_outside_touch_files() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/allowed.rs"), "base\n").unwrap();
        std::fs::write(main.join("src/other.rs"), "base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("src/other.rs"), "changed\n").unwrap();

        let violations = detect_contract_violations(
            &task,
            &baseline,
            &["src/allowed.rs".to_string()],
            &["src/".to_string()],
            &[],
        )
        .unwrap();

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].path, std::path::PathBuf::from("src/other.rs"));
        assert!(violations[0].reason.contains("not in touchFiles"));
    }

    #[test]
    fn detect_contract_violations_accepts_cargo_lock_companion_with_absolute_touch_file() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("Cargo.toml"), "[package]\nname = \"libra\"\n").unwrap();
        std::fs::write(main.join("src/main.rs"), "fn main() {}\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("Cargo.lock"), "# generated lockfile\n").unwrap();

        // Simulates touch_files coming straight from the LLM with absolute paths;
        // the cargo-lock companion match should still tolerate it.
        let violations = detect_contract_violations(
            &task,
            &baseline,
            &[
                "/some/abs/Cargo.toml".to_string(),
                "/some/abs/src/main.rs".to_string(),
            ],
            &[],
            &[],
        )
        .unwrap();

        assert!(
            violations.is_empty(),
            "expected no violations, got {:?}",
            violations
        );
    }

    #[test]
    fn sync_rejects_changes_outside_write_scope_when_touch_files_absent() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("main");
        let task = temp.path().join("task");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::create_dir_all(main.join("docs")).unwrap();
        std::fs::write(main.join("src/allowed.rs"), "base\n").unwrap();
        std::fs::write(main.join("docs/readme.md"), "base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        std::fs::create_dir_all(&task).unwrap();
        materialize_workspace(&main, &task, &baseline).unwrap();
        std::fs::write(task.join("docs/readme.md"), "changed\n").unwrap();

        let err = sync_task_worktree_back(&main, &task, &baseline, &[], &["src/".to_string()], &[])
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("path 'docs/readme.md' not in any in-scope pattern")
        );
        assert_eq!(
            std::fs::read_to_string(main.join("docs/readme.md")).unwrap(),
            "base\n"
        );
    }

    #[test]
    fn prepare_task_worktree_supports_plain_directories_without_repo_storage() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("workspace");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "fn main() {}\n").unwrap();

        let (task_worktree, _) =
            prepare_task_worktree(&main, Uuid::new_v4(), &FuseProvisionState::default()).unwrap();

        assert_eq!(
            std::fs::read_to_string(task_worktree.root.join("src/lib.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(!task_worktree.root.join(util::ROOT_DIR).exists());

        cleanup_task_worktree(task_worktree).unwrap();
    }

    #[test]
    fn prepare_task_worktree_skips_gitignored_build_outputs() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("workspace");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::create_dir_all(main.join("target/debug")).unwrap();
        std::fs::write(main.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(main.join("src/lib.rs"), "fn main() {}\n").unwrap();
        std::fs::write(main.join("target/debug/app"), "compiled\n").unwrap();

        let (task_worktree, _) =
            prepare_task_worktree(&main, Uuid::new_v4(), &FuseProvisionState::default()).unwrap();

        assert!(task_worktree.root.join("src/lib.rs").exists());
        assert!(!task_worktree.root.join("target").exists());

        cleanup_task_worktree(task_worktree).unwrap();
    }

    #[test]
    fn fuse_provision_state_defaults_disabled_in_unit_tests() {
        assert!(FuseProvisionState::default().is_disabled());
    }

    #[test]
    fn device_not_configured_is_classified_as_fuse_infrastructure_error() {
        assert!(super::is_fuse_infrastructure_error_message(
            "Tool 'read_file' failed: Device not configured (os error 6)"
        ));
        assert!(super::is_fuse_infrastructure_error_message(
            "failed to snapshot worktree: os error 6"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn fuse_health_check_allows_plain_workspace_without_repo_storage() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        super::verify_fuse_task_worktree_mount(&workspace, false).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fuse_health_check_reports_missing_repo_storage_context() {
        let temp = tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let err = super::verify_fuse_task_worktree_mount(&workspace, true).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("FUSE mount health check failed after"));
        assert!(message.contains("expected .libra repository storage link is not visible"));
        assert!(message.contains(workspace.to_string_lossy().as_ref()));
    }

    #[test]
    fn prepare_copy_task_worktree_includes_untracked_workspace_files() {
        let temp = tempdir().unwrap();
        let main = temp.path().join("workspace");
        std::fs::create_dir_all(main.join("src")).unwrap();
        std::fs::write(main.join("src/lib.rs"), "fn main() {}\n").unwrap();
        std::fs::write(main.join("task_a.txt"), "base\n").unwrap();
        std::fs::write(main.join("task_b.txt"), "base\n").unwrap();

        let baseline = snapshot_workspace(&main).unwrap();
        let paths = task_worktree_paths(Uuid::new_v4(), "copy-test");
        prepare_task_worktree_root(&paths.cleanup_root).unwrap();

        let backend = prepare_copy_task_worktree(&main, &paths, &baseline).unwrap();

        assert!(matches!(backend, super::TaskWorktreeBackend::Copy { .. }));
        assert_eq!(
            std::fs::read_to_string(paths.workspace_root.join("task_a.txt")).unwrap(),
            "base\n"
        );
        assert_eq!(
            std::fs::read_to_string(paths.workspace_root.join("task_b.txt")).unwrap(),
            "base\n"
        );

        cleanup_task_worktree(super::TaskWorktree {
            root: paths.workspace_root.clone(),
            baseline,
            backend,
        })
        .unwrap();
    }

    #[tokio::test]
    async fn prepare_task_worktree_keeps_repo_storage_visible_in_runtime() {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        test::setup_with_new_libra_in(&repo).await;
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/lib.rs"), "pub fn worktree() {}\n").unwrap();

        let repo_for_prepare = repo.clone();
        let (task_worktree, _) = tokio::task::spawn_blocking(move || {
            prepare_task_worktree(
                &repo_for_prepare,
                Uuid::new_v4(),
                &FuseProvisionState::default(),
            )
        })
        .await
        .unwrap()
        .unwrap();

        assert!(task_worktree.root.join(util::ROOT_DIR).exists());
        assert_eq!(
            std::fs::read_to_string(task_worktree.root.join("src/lib.rs")).unwrap(),
            "pub fn worktree() {}\n"
        );

        tokio::task::spawn_blocking(move || cleanup_task_worktree(task_worktree))
            .await
            .unwrap()
            .unwrap();
    }
}
