//! `libra worktree` command implementation for mounting worktree overlays.
//!
//! Boundary: this command is Unix-only and focuses on FUSE mount lifecycle; generic
//! worktree management remains in `command::worktree`. Worktree-fuse command tests
//! cover argument parsing and unsupported-platform behavior.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use clap::{Parser, Subcommand};
use libfuse_fs::overlayfs::{OverlayArgs, mount_fs};
use rfuse3::raw::MountHandle;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[path = "worktree.rs"]
mod legacy;

use crate::{
    command::{
        branch,
        restore::{self, RestoreArgs},
    },
    internal::head::Head,
    utils::{
        error::{CliError, CliResult},
        fuse as fuse_utils,
        output::OutputConfig,
        util,
    },
};

#[derive(Parser, Debug)]
pub struct WorktreeArgs {
    #[clap(subcommand)]
    pub command: WorktreeSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum WorktreeSubcommand {
    Add {
        path: String,
        #[clap(short = 'f', long, help = "Use FUSE overlay worktree mode (Unix only)")]
        fuse: bool,
        #[clap(long, help = "Checkout this branch in the new worktree")]
        branch: Option<String>,
        #[clap(
            short = 'b',
            long = "create-branch",
            help = "Create and checkout a new branch"
        )]
        create_branch: Option<String>,
        #[clap(
            long,
            conflicts_with = "create_branch",
            help = "Base ref for --create-branch"
        )]
        from: Option<String>,
        #[clap(long, help = "Use privileged mount mode")]
        privileged: bool,
        #[clap(long, help = "Allow other users to access the mounted worktree")]
        allow_other: bool,
    },
    List,
    Lock {
        path: String,
        #[clap(long)]
        reason: Option<String>,
    },
    Unlock {
        path: String,
    },
    Move {
        src: String,
        dest: String,
    },
    Prune,
    Remove {
        path: String,
    },
    #[clap(alias = "unmount", about = "Unmount a FUSE worktree mountpoint")]
    Umount {
        path: String,
        #[clap(
            long,
            help = "Remove the Libra task worktree root after unmounting its workspace mountpoint"
        )]
        cleanup: bool,
    },
    Repair,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct FuseWorktreeEntry {
    path: String,
    branch: String,
    upper_dir: String,
    lower_dirs: Vec<String>,
    locked: bool,
    lock_reason: Option<String>,
    privileged: bool,
    allow_other: bool,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
struct FuseWorktreeState {
    worktrees: Vec<FuseWorktreeEntry>,
}

trait IntoMountHandleResult {
    fn into_mount_handle_result(self) -> io::Result<MountHandle>;
}

impl IntoMountHandleResult for MountHandle {
    fn into_mount_handle_result(self) -> io::Result<MountHandle> {
        Ok(self)
    }
}

impl<E> IntoMountHandleResult for Result<MountHandle, E>
where
    E: std::fmt::Display,
{
    fn into_mount_handle_result(self) -> io::Result<MountHandle> {
        self.map_err(|e| io::Error::other(format!("failed to mount FUSE worktree: {e}")))
    }
}

fn active_mounts() -> &'static Mutex<HashMap<String, MountHandle>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, MountHandle>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fuse_state_lock() -> &'static Mutex<()> {
    static STATE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    STATE_LOCK.get_or_init(|| Mutex::new(()))
}

/// Executes the worktree command in user-facing mode.
///
/// This wrapper delegates to [`execute_safe`] and prints any returned
/// [`CliError`] to stderr instead of propagating it to the caller.
/// Use this entry when the command is invoked from normal CLI dispatch.
pub async fn execute(args: WorktreeArgs) {
    if let Err(e) = execute_safe(args, &OutputConfig::default()).await {
        e.print_stderr();
    }
}

/// Executes the worktree command and returns structured errors.
///
/// Behavior summary:
/// - Ensures the current directory is a Libra repository.
/// - Routes `add --fuse`, `list`, `lock`, `unlock`, `remove`, `prune`, and
///   `repair` through FUSE-aware logic.
/// - Falls back to legacy worktree implementation for non-FUSE paths and
///   operations not implemented in the FUSE layer.
/// - Validates that `--branch`/`--create-branch`/`--from` are used only with
///   `--fuse`.
///
/// Returns [`CliResult<()>`] so callers can decide whether to bubble up,
/// map, or render failures.
pub async fn execute_safe(args: WorktreeArgs, output: &OutputConfig) -> CliResult<()> {
    let command = args.command;
    if !matches!(&command, WorktreeSubcommand::Umount { .. }) {
        util::require_repo().map_err(|_| CliError::repo_not_found())?;
    }

    match command {
        WorktreeSubcommand::Add {
            path,
            fuse,
            branch,
            create_branch,
            from,
            privileged,
            allow_other,
        } => {
            if !fuse {
                if branch.is_some() || create_branch.is_some() || from.is_some() {
                    return Err(CliError::command_usage(
                        "--branch/--create-branch/--from require --fuse",
                    ));
                }
                legacy::execute_safe(
                    legacy::WorktreeArgs {
                        command: legacy::WorktreeSubcommand::Add { path },
                    },
                    output,
                )
                .await
            } else {
                add_fuse_worktree(path, branch, create_branch, from, privileged, allow_other)
                    .await
                    .map_err(|e| CliError::fatal(e.to_string()))
            }
        }
        WorktreeSubcommand::List => list_all_worktrees(output)
            .await
            .map_err(|e| CliError::fatal(e.to_string())),
        WorktreeSubcommand::Lock { path, reason } => {
            if lock_fuse_worktree(&path, reason.clone())
                .map_err(|e| CliError::fatal(e.to_string()))?
            {
                return Ok(());
            }
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Lock { path, reason },
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Unlock { path } => {
            if unlock_fuse_worktree(&path).map_err(|e| CliError::fatal(e.to_string()))? {
                return Ok(());
            }
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Unlock { path },
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Remove { path } => {
            if remove_fuse_worktree(&path)
                .await
                .map_err(|e| CliError::fatal(e.to_string()))?
            {
                return Ok(());
            }
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Remove { path },
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Umount { path, cleanup } => umount_fuse_path(path, cleanup)
            .await
            .map_err(|e| CliError::fatal(e.to_string())),
        WorktreeSubcommand::Move { src, dest } => {
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Move { src, dest },
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Prune => {
            prune_fuse_worktrees().map_err(|e| CliError::fatal(e.to_string()))?;
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Prune,
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Repair => {
            repair_fuse_worktrees().map_err(|e| CliError::fatal(e.to_string()))?;
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Repair,
                },
                output,
            )
            .await
        }
    }
}

fn canonicalize_like_worktree<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let p = path.as_ref();
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        util::cur_dir().join(p)
    };
    let normalized = fuse_utils::normalize_abs_path(&joined);
    if normalized.exists() {
        fs::canonicalize(normalized)
    } else {
        Ok(normalized)
    }
}

fn fuse_state_path() -> PathBuf {
    util::storage_path().join("worktrees-fuse.json")
}

fn fuse_data_root() -> PathBuf {
    util::storage_path().join("worktrees-fuse")
}

fn load_fuse_state() -> io::Result<FuseWorktreeState> {
    let path = fuse_state_path();
    if !path.exists() {
        return Ok(FuseWorktreeState::default());
    }
    let data = fs::read(&path)?;
    if data.is_empty() {
        return Ok(FuseWorktreeState::default());
    }
    let state: FuseWorktreeState =
        serde_json::from_slice(&data).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(state)
}

fn save_fuse_state(state: &FuseWorktreeState) -> io::Result<()> {
    let path = fuse_state_path();
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(state).map_err(|e| io::Error::other(e.to_string()))?;
    fs::write(&tmp, data)?;
    #[cfg(windows)]
    {
        if path.exists() {
            let _ = fs::remove_file(&path);
        }
    }
    fs::rename(tmp, path)
}

fn verify_mount_health(mountpoint: &Path) -> io::Result<()> {
    fs::read_dir(mountpoint)?;
    Ok(())
}

async fn add_fuse_worktree(
    path: String,
    branch_name: Option<String>,
    create_branch_name: Option<String>,
    from: Option<String>,
    privileged: bool,
    allow_other: bool,
) -> io::Result<()> {
    let storage = util::storage_path();
    let target = canonicalize_like_worktree(&path)?;

    if util::is_sub_path(&target, &storage) {
        return Err(io::Error::other(
            "worktree path cannot be inside .libra storage",
        ));
    }

    let target_exists = target.exists();
    if target_exists && !target.is_dir() {
        return Err(io::Error::other("target exists and is not a directory"));
    }
    if target_exists && fs::read_dir(&target)?.next().transpose()?.is_some() {
        return Err(io::Error::other("target directory exists and is not empty"));
    }

    let state = load_fuse_state()?;
    if state.worktrees.iter().any(|w| Path::new(&w.path) == target) {
        println!("worktree already exists at {}", target.display());
        return Ok(());
    }

    if let Some(new_branch) = create_branch_name.as_ref() {
        branch::create_branch_safe(new_branch.clone(), from.clone())
            .await
            .map_err(|e| io::Error::other(format!("failed to create branch: {e}")))?;
    }

    let checkout_branch = if let Some(name) = create_branch_name.clone().or(branch_name) {
        name
    } else {
        match Head::current().await {
            Head::Branch(name) => name,
            _ => "HEAD".to_string(),
        }
    };

    let mut created_target = false;
    if !target.exists() {
        fs::create_dir_all(&target)?;
        created_target = true;
    }

    let id = Uuid::new_v4().simple().to_string();
    let upper_dir = fuse_data_root().join(id).join("upper");
    fs::create_dir_all(&upper_dir)?;

    let lower_dir = canonicalize_like_worktree(util::working_dir())?;
    let mount_args = OverlayArgs {
        mountpoint: &target,
        upperdir: &upper_dir,
        lowerdir: vec![lower_dir.clone()],
        privileged,
        mapping: None::<&str>,
        name: Some("libra-worktree-fuse"),
        allow_other,
    };
    let mount_handle = mount_fs(mount_args).await.into_mount_handle_result()?;

    if let Err(err) = verify_mount_health(&target) {
        let _ = mount_handle.unmount().await;
        let _ = fs::remove_dir_all(&upper_dir);
        if created_target {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(io::Error::other(format!(
            "FUSE mount health check failed: {err}"
        )));
    }

    let mut rollback_needed = true;
    if Head::current_commit().await.is_some()
        && let Err(err) = restore::execute_checked(RestoreArgs {
            pathspec: vec![target.to_string_lossy().to_string()],
            source: Some(checkout_branch.clone()),
            worktree: true,
            staged: false,
        })
        .await
    {
        let _ = mount_handle.unmount().await;
        let _ = fs::remove_dir_all(&upper_dir);
        if created_target {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(io::Error::other(format!(
            "failed to populate FUSE worktree from '{}': {err}",
            checkout_branch
        )));
    }

    if let Ok(mut mounts) = active_mounts().lock() {
        mounts.insert(target.to_string_lossy().to_string(), mount_handle);
    } else {
        rollback_needed = false;
    }

    let save_result = {
        let _guard = fuse_state_lock()
            .lock()
            .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
        let mut current = load_fuse_state()?;
        if current
            .worktrees
            .iter()
            .any(|w| Path::new(&w.path) == target)
        {
            Ok(())
        } else {
            current.worktrees.push(FuseWorktreeEntry {
                path: target.to_string_lossy().to_string(),
                branch: checkout_branch,
                upper_dir: upper_dir.to_string_lossy().to_string(),
                lower_dirs: vec![lower_dir.to_string_lossy().to_string()],
                locked: false,
                lock_reason: None,
                privileged,
                allow_other,
            });
            save_fuse_state(&current)
        }
    };

    if let Err(err) = save_result {
        if rollback_needed {
            let _ = unmount_path(&target).await;
        }
        let _ = fs::remove_dir_all(&upper_dir);
        if created_target {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(err);
    }

    println!("{}", target.display());
    Ok(())
}

async fn list_all_worktrees(output: &OutputConfig) -> io::Result<()> {
    legacy::execute_safe(
        legacy::WorktreeArgs {
            command: legacy::WorktreeSubcommand::List,
        },
        output,
    )
    .await
    .map_err(|e| io::Error::other(e.to_string()))?;

    let state = load_fuse_state()?;
    for entry in state.worktrees {
        let mounted = if fuse_utils::is_mount_active(Path::new(&entry.path)) {
            "mounted"
        } else {
            "unmounted"
        };
        let mut line = format!(
            "worktree {} [branch: {}] [fuse: {}]",
            entry.path, entry.branch, mounted
        );
        if entry.locked {
            line.push_str(" [locked");
            if let Some(reason) = entry.lock_reason.as_ref()
                && !reason.is_empty()
            {
                line.push_str(": ");
                line.push_str(reason);
            }
            line.push(']');
        }
        println!("{}", line);
    }

    Ok(())
}

fn lock_fuse_worktree(path: &str, reason: Option<String>) -> io::Result<bool> {
    let _state_guard = fuse_state_lock()
        .lock()
        .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
    let target = canonicalize_like_worktree(path)?;
    let mut state = load_fuse_state()?;
    let mut changed = false;
    let mut found = false;
    for worktree in &mut state.worktrees {
        if Path::new(&worktree.path) == target {
            found = true;
            if !worktree.locked {
                worktree.locked = true;
                worktree.lock_reason = reason;
                changed = true;
            }
            break;
        }
    }
    if found && changed {
        save_fuse_state(&state)?;
    }
    Ok(found)
}

fn unlock_fuse_worktree(path: &str) -> io::Result<bool> {
    let _state_guard = fuse_state_lock()
        .lock()
        .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
    let target = canonicalize_like_worktree(path)?;
    let mut state = load_fuse_state()?;
    let mut changed = false;
    let mut found = false;
    for worktree in &mut state.worktrees {
        if Path::new(&worktree.path) == target {
            found = true;
            if worktree.locked {
                worktree.locked = false;
                worktree.lock_reason = None;
                changed = true;
            }
            break;
        }
    }
    if found && changed {
        save_fuse_state(&state)?;
    }
    Ok(found)
}

async fn remove_fuse_worktree(path: &str) -> io::Result<bool> {
    let target = canonicalize_like_worktree(path)?;
    let entry = {
        let _state_guard = fuse_state_lock()
            .lock()
            .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
        let state = load_fuse_state()?;
        let Some(found) = state
            .worktrees
            .iter()
            .find(|w| Path::new(&w.path) == target)
            .cloned()
        else {
            return Ok(false);
        };
        found
    };

    if entry.locked {
        return Err(io::Error::other("cannot remove locked worktree"));
    }

    if let Err(err) = unmount_path(&target).await
        && fuse_utils::is_mount_active(&target)
    {
        return Err(err);
    }
    if Path::new(&entry.upper_dir).exists() {
        fs::remove_dir_all(&entry.upper_dir)?;
    }
    {
        let _state_guard = fuse_state_lock()
            .lock()
            .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
        let mut state = load_fuse_state()?;
        if let Some(index) = state
            .worktrees
            .iter()
            .position(|w| Path::new(&w.path) == target)
        {
            state.worktrees.remove(index);
            save_fuse_state(&state)?;
        }
    }
    Ok(true)
}

fn prune_fuse_worktrees() -> io::Result<()> {
    let _state_guard = fuse_state_lock()
        .lock()
        .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
    let mut state = load_fuse_state()?;
    let before = state.worktrees.len();
    state.worktrees.retain(|entry| {
        let path = Path::new(&entry.path);
        path.exists() || entry.locked
    });
    if state.worktrees.len() != before {
        save_fuse_state(&state)?;
    }
    Ok(())
}

fn repair_fuse_worktrees() -> io::Result<()> {
    let _state_guard = fuse_state_lock()
        .lock()
        .map_err(|_| io::Error::other("fuse state lock poisoned"))?;
    let mut state = load_fuse_state()?;
    let mut seen = std::collections::HashSet::<PathBuf>::new();
    let before = state.worktrees.len();
    state.worktrees.retain(|entry| {
        let p = PathBuf::from(&entry.path);
        seen.insert(p)
    });
    if state.worktrees.len() != before {
        save_fuse_state(&state)?;
    }
    Ok(())
}

async fn unmount_path(path: &Path) -> io::Result<()> {
    let path = fuse_utils::normalize_abs_path(path);
    let handle = active_mounts()
        .lock()
        .ok()
        .and_then(|mut mounts| mounts.remove(&path.to_string_lossy().to_string()));
    if let Some(handle) = handle {
        match handle.unmount().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let ioe: io::Error = e;
                if matches!(
                    ioe.raw_os_error(),
                    Some(libc::ENOTCONN | libc::EINVAL | libc::ENOENT | libc::EPERM)
                ) {
                    if !fuse_utils::is_mount_active(&path) {
                        return Ok(());
                    }
                } else {
                    return Err(io::Error::other(format!(
                        "failed to unmount FUSE worktree: {ioe}"
                    )));
                }
            }
        }
    }

    fuse_utils::force_unmount_path(&path)
}

async fn umount_fuse_path(path: String, cleanup: bool) -> io::Result<()> {
    let target = canonicalize_like_worktree(path)?;
    let mountpoint = fuse_utils::resolve_task_worktree_mountpoint_arg(&target);
    unmount_path(&mountpoint).await.map_err(|err| {
        io::Error::new(
            err.kind(),
            format!(
                "failed to unmount FUSE path {}: {}",
                mountpoint.display(),
                err
            ),
        )
    })?;
    println!("unmounted {}", mountpoint.display());

    if cleanup {
        let cleanup_root =
            fuse_utils::fuse_task_worktree_cleanup_root(&mountpoint).ok_or_else(|| {
                io::Error::other(format!(
                    "--cleanup only supports Libra task FUSE worktree paths ending in '/workspace': {}",
                    mountpoint.display()
                ))
            })?;
        if cleanup_root.exists() {
            fs::remove_dir_all(&cleanup_root)?;
        }
        println!("removed {}", cleanup_root.display());
    }

    Ok(())
}
