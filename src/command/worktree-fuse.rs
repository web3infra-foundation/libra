//! `libra worktree` command implementation for mounting worktree overlays.
//!
//! Boundary: this command is Unix-only and focuses on FUSE mount lifecycle; generic
//! worktree management remains in `command::worktree`. Worktree-fuse command tests
//! cover argument parsing and unsupported-platform behavior.

#[cfg(target_os = "macos")]
use std::env;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
#[cfg(not(target_os = "macos"))]
use libfuse_fs::overlayfs::{OverlayFs, config::Config as FuseOverlayConfig};
use libfuse_fs::passthrough::{PassthroughArgs, new_passthroughfs_layer};
use rfuse3::{
    MountOptions,
    raw::{MountHandle, Session},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[path = "worktree.rs"]
mod legacy;

// Re-export the shared `--help` examples constant so the cli definition can
// reference `command::worktree::WORKTREE_EXAMPLES` regardless of whether the
// `worktree-fuse` feature routed compilation through this file or directly
// through `worktree.rs`.
pub use legacy::WORKTREE_EXAMPLES;

use crate::{
    command::{
        branch,
        restore::{self, RestoreArgs},
    },
    internal::head::Head,
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        fuse as fuse_utils,
        output::{OutputConfig, emit_json_data},
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
        #[clap(long, help = "Also delete the worktree directory on disk")]
        delete_dir: bool,
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
    #[clap(hide = true, name = "__fuse-daemon")]
    FuseDaemon {
        mountpoint: String,
        upper_dir: String,
        lower_dir: String,
        #[clap(long)]
        privileged: bool,
        #[clap(long)]
        allow_other: bool,
    },
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

#[derive(Debug, Serialize)]
struct WorktreeUmountOutput {
    mountpoint: String,
    unmounted: bool,
    cleanup_requested: bool,
    cleanup_root: Option<String>,
    cleanup_root_removed: bool,
}

#[derive(Debug)]
enum FuseUmountError {
    InvalidTarget(String),
    IoRead(String),
    IoWrite(String),
}

impl FuseUmountError {
    fn stable_code(&self) -> StableErrorCode {
        match self {
            Self::InvalidTarget(_) => StableErrorCode::CliInvalidTarget,
            Self::IoRead(_) => StableErrorCode::IoReadFailed,
            Self::IoWrite(_) => StableErrorCode::IoWriteFailed,
        }
    }

    fn into_cli_error(self) -> CliError {
        CliError::fatal(self.to_string()).with_stable_code(self.stable_code())
    }
}

impl std::fmt::Display for FuseUmountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget(message) | Self::IoRead(message) | Self::IoWrite(message) => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for FuseUmountError {}

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
        WorktreeSubcommand::FuseDaemon {
            mountpoint,
            upper_dir,
            lower_dir,
            privileged,
            allow_other,
        } => run_fuse_daemon(mountpoint, upper_dir, lower_dir, privileged, allow_other)
            .await
            .map_err(|e| CliError::fatal(e.to_string())),
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
        WorktreeSubcommand::Remove { path, delete_dir } => {
            if remove_fuse_worktree(&path)
                .await
                .map_err(|e| CliError::fatal(e.to_string()))?
            {
                return Ok(());
            }
            legacy::execute_safe(
                legacy::WorktreeArgs {
                    command: legacy::WorktreeSubcommand::Remove { path, delete_dir },
                },
                output,
            )
            .await
        }
        WorktreeSubcommand::Umount { path, cleanup } => {
            let result = umount_fuse_path(path, cleanup)
                .await
                .map_err(FuseUmountError::into_cli_error)?;
            render_umount_fuse_path(&result, output)
        }
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

#[cfg(target_os = "macos")]
struct DirGuard {
    old_dir: PathBuf,
}

#[cfg(target_os = "macos")]
impl DirGuard {
    fn change_to(new_dir: &Path) -> io::Result<Self> {
        let old_dir = env::current_dir()?;
        env::set_current_dir(new_dir)?;
        Ok(Self { old_dir })
    }
}

#[cfg(target_os = "macos")]
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = env::set_current_dir(&self.old_dir);
    }
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

fn verify_mount_health_until(mountpoint: &Path) -> io::Result<()> {
    let started = Instant::now();
    let mut last_error = None;
    while started.elapsed() < Duration::from_secs(5) {
        if fuse_utils::is_mount_active(mountpoint) {
            match verify_mount_health(mountpoint) {
                Ok(()) => return Ok(()),
                Err(err) => last_error = Some(err),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "FUSE mount did not become healthy at {}",
                mountpoint.display()
            ),
        )
    }))
}

async fn mount_fuse_overlay(
    mountpoint: &Path,
    upper_dir: &Path,
    lower_dirs: &[PathBuf],
    privileged: bool,
    allow_other: bool,
) -> io::Result<MountHandle> {
    #[cfg(target_os = "macos")]
    {
        let _ = lower_dirs;
        let fs = new_passthroughfs_layer(PassthroughArgs {
            root_dir: upper_dir,
            mapping: None::<&str>,
        })
        .await
        .map_err(|err| {
            io::Error::other(format!("failed to create FUSE passthrough layer: {err}"))
        })?;

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let mut mount_options = MountOptions::default();
        mount_options
            .uid(uid)
            .gid(gid)
            .fs_name("libra-worktree-fuse");
        if privileged || allow_other {
            mount_options.allow_other(allow_other);
        }

        return if privileged {
            Session::new(mount_options)
                .mount(fs, mountpoint.as_os_str())
                .await
        } else {
            Session::new(mount_options)
                .mount_with_unprivileged(fs, mountpoint.as_os_str())
                .await
        }
        .map_err(|err| io::Error::other(format!("failed to mount FUSE passthrough: {err}")));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let upper_layer = std::sync::Arc::new(
            new_passthroughfs_layer(PassthroughArgs {
                root_dir: upper_dir,
                mapping: None::<&str>,
            })
            .await
            .map_err(|err| io::Error::other(format!("failed to create FUSE upper layer: {err}")))?,
        );

        let mut lower_layers = Vec::with_capacity(lower_dirs.len());
        for lower_dir in lower_dirs {
            lower_layers.push(std::sync::Arc::new(
                new_passthroughfs_layer(PassthroughArgs {
                    root_dir: lower_dir,
                    mapping: None::<&str>,
                })
                .await
                .map_err(|err| {
                    io::Error::other(format!("failed to create FUSE lower layer: {err}"))
                })?,
            ));
        }

        let overlay = OverlayFs::new(
            Some(upper_layer),
            lower_layers,
            FuseOverlayConfig {
                mountpoint: mountpoint.to_path_buf(),
                do_import: true,
                writeback: true,
                ..Default::default()
            },
            1,
        )
        .map_err(|err| io::Error::other(format!("failed to create FUSE overlay: {err}")))?;
        overlay.import().await.map_err(|err| {
            io::Error::other(format!("failed to import FUSE overlay layers: {err}"))
        })?;

        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        let mut mount_options = MountOptions::default();
        #[cfg(target_os = "linux")]
        mount_options.force_readdir_plus(true);
        mount_options
            .uid(uid)
            .gid(gid)
            .fs_name("libra-worktree-fuse");
        if privileged || allow_other {
            mount_options.allow_other(allow_other);
        }

        if privileged {
            Session::new(mount_options)
                .mount(overlay, mountpoint.as_os_str())
                .await
        } else {
            Session::new(mount_options)
                .mount_with_unprivileged(overlay, mountpoint.as_os_str())
                .await
        }
        .map_err(|err| io::Error::other(format!("failed to mount FUSE overlay: {err}")))
    }
}

async fn run_fuse_daemon(
    mountpoint: String,
    upper_dir: String,
    lower_dir: String,
    privileged: bool,
    allow_other: bool,
) -> io::Result<()> {
    let mountpoint = PathBuf::from(mountpoint);
    let upper_dir = PathBuf::from(upper_dir);
    let lower_dirs = vec![PathBuf::from(lower_dir)];
    let _mount_handle = mount_fuse_overlay(
        &mountpoint,
        &upper_dir,
        &lower_dirs,
        privileged,
        allow_other,
    )
    .await?;
    verify_mount_health_until(&mountpoint)?;
    futures::future::pending::<()>().await;
    Ok(())
}

#[cfg(target_os = "macos")]
fn spawn_macos_fuse_daemon(
    mountpoint: &Path,
    upper_dir: &Path,
    lower_dirs: &[PathBuf],
    privileged: bool,
    allow_other: bool,
) -> io::Result<()> {
    let lower_dir = lower_dirs
        .first()
        .ok_or_else(|| io::Error::other("macOS FUSE daemon requires a lower layer"))?;
    let current_exe = env::current_exe()?;
    Command::new(current_exe)
        .arg("worktree")
        .arg("__fuse-daemon")
        .arg(mountpoint)
        .arg(upper_dir)
        .arg(lower_dir)
        .args(privileged.then_some("--privileged"))
        .args(allow_other.then_some("--allow-other"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
async fn populate_macos_fuse_upper_dir(upper_dir: &Path, checkout_branch: &str) -> io::Result<()> {
    if Head::current_commit().await.is_none() {
        return Ok(());
    }

    link_repo_storage_into_dir(upper_dir)?;
    let _guard = DirGuard::change_to(upper_dir)?;
    restore::execute_checked(RestoreArgs {
        pathspec: vec![util::working_dir_string()],
        source: Some(checkout_branch.to_string()),
        worktree: true,
        staged: false,
        ..Default::default()
    })
    .await
    .map_err(|err| {
        io::Error::other(format!(
            "failed to populate macOS FUSE upper layer from '{}': {err}",
            checkout_branch
        ))
    })
}

#[cfg(target_os = "macos")]
fn link_repo_storage_into_dir(dir: &Path) -> io::Result<()> {
    let storage = util::storage_path();
    let link_path = dir.join(util::ROOT_DIR);
    if link_path.exists() {
        return Ok(());
    }
    std::os::unix::fs::symlink(storage, link_path)
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
    let data_dir = fuse_data_root().join(id);
    let upper_dir = data_dir.join("upper");
    #[cfg(target_os = "macos")]
    let lower_dir = data_dir.join("lower");
    fs::create_dir_all(&upper_dir)?;

    #[cfg(target_os = "macos")]
    {
        fs::create_dir_all(&lower_dir)?;
        if let Err(err) = populate_macos_fuse_upper_dir(&upper_dir, &checkout_branch).await {
            let _ = fs::remove_dir_all(&data_dir);
            if created_target {
                let _ = fs::remove_dir_all(&target);
            }
            return Err(err);
        }
    }

    #[cfg(target_os = "macos")]
    let lower_dirs = vec![lower_dir.clone()];
    #[cfg(not(target_os = "macos"))]
    let lower_dirs = vec![canonicalize_like_worktree(util::working_dir())?];

    #[cfg(target_os = "macos")]
    let mount_handle: Option<MountHandle> = {
        spawn_macos_fuse_daemon(&target, &upper_dir, &lower_dirs, privileged, allow_other)?;
        verify_mount_health_until(&target)?;
        None
    };

    #[cfg(not(target_os = "macos"))]
    let mount_handle =
        Some(mount_fuse_overlay(&target, &upper_dir, &lower_dirs, privileged, allow_other).await?);

    if let Err(err) = verify_mount_health(&target) {
        if let Some(mount_handle) = mount_handle {
            let _ = mount_handle.unmount().await;
        } else {
            let _ = fuse_utils::force_unmount_path(&target);
        }
        let _ = fs::remove_dir_all(&data_dir);
        if created_target {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(io::Error::other(format!(
            "FUSE mount health check failed: {err}"
        )));
    }

    let mut rollback_needed = true;
    if cfg!(not(target_os = "macos"))
        && Head::current_commit().await.is_some()
        && let Err(err) = restore::execute_checked(RestoreArgs {
            pathspec: vec![target.to_string_lossy().to_string()],
            source: Some(checkout_branch.clone()),
            worktree: true,
            staged: false,
            ..Default::default()
        })
        .await
    {
        if let Some(mount_handle) = mount_handle {
            let _ = mount_handle.unmount().await;
        } else {
            let _ = fuse_utils::force_unmount_path(&target);
        }
        let _ = fs::remove_dir_all(&data_dir);
        if created_target {
            let _ = fs::remove_dir_all(&target);
        }
        return Err(io::Error::other(format!(
            "failed to populate FUSE worktree from '{}': {err}",
            checkout_branch
        )));
    }

    if let Some(mount_handle) = mount_handle {
        if let Ok(mut mounts) = active_mounts().lock() {
            mounts.insert(target.to_string_lossy().to_string(), mount_handle);
        } else {
            rollback_needed = false;
        }
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
                lower_dirs: lower_dirs
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect(),
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
        let _ = fs::remove_dir_all(&data_dir);
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

async fn umount_fuse_path(
    path: String,
    cleanup: bool,
) -> Result<WorktreeUmountOutput, FuseUmountError> {
    let target = canonicalize_like_worktree(&path).map_err(|source| {
        FuseUmountError::IoRead(format!(
            "failed to resolve FUSE worktree path '{}': {source}",
            path
        ))
    })?;
    let mountpoint = fuse_utils::resolve_task_worktree_mountpoint_arg(&target);
    unmount_path(&mountpoint).await.map_err(|source| {
        FuseUmountError::IoWrite(format!(
            "failed to unmount FUSE path {}: {source}",
            mountpoint.display()
        ))
    })?;

    let mut cleanup_root = None;
    let mut cleanup_root_removed = false;
    if cleanup {
        let root = fuse_utils::fuse_task_worktree_cleanup_root(&mountpoint).ok_or_else(|| {
            FuseUmountError::InvalidTarget(format!(
                "--cleanup only supports Libra task FUSE worktree paths ending in '/workspace': {}",
                mountpoint.display()
            ))
        })?;
        if root.exists() {
            fs::remove_dir_all(&root).map_err(|source| {
                FuseUmountError::IoWrite(format!(
                    "failed to remove FUSE worktree root '{}': {source}",
                    root.display()
                ))
            })?;
            cleanup_root_removed = true;
        }
        cleanup_root = Some(root.to_string_lossy().to_string());
    }

    Ok(WorktreeUmountOutput {
        mountpoint: mountpoint.to_string_lossy().to_string(),
        unmounted: true,
        cleanup_requested: cleanup,
        cleanup_root,
        cleanup_root_removed,
    })
}

fn render_umount_fuse_path(result: &WorktreeUmountOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("worktree.umount", result, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!("unmounted {}", result.mountpoint);
    if let Some(cleanup_root) = &result.cleanup_root {
        println!("removed {}", cleanup_root);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the `stable_code()` mapping for every variant of
    /// [`FuseUmountError`]. JSON consumers branch on the
    /// [`StableErrorCode`] in the error envelope for `libra worktree
    /// umount`. The Display body just echoes the inner string
    /// verbatim regardless of variant, so this is the only
    /// public surface contract worth pinning per-variant — a future
    /// refactor that flipped (say) `IoRead` from `IoReadFailed` to
    /// the catch-all `Other` code would silently change the wire
    /// surface unless every variant has its own guard.
    ///
    /// Continuation of the post-v0.17.700 surface-contract sweep
    /// (TuiControlError / CherryPickError / RevertError /
    /// RestoreError / StashError / ResetError).
    #[test]
    fn fuse_umount_error_stable_code_pins_each_variant() {
        assert_eq!(
            FuseUmountError::InvalidTarget("ignored".to_string()).stable_code(),
            StableErrorCode::CliInvalidTarget,
        );
        assert_eq!(
            FuseUmountError::IoRead("ignored".to_string()).stable_code(),
            StableErrorCode::IoReadFailed,
        );
        assert_eq!(
            FuseUmountError::IoWrite("ignored".to_string()).stable_code(),
            StableErrorCode::IoWriteFailed,
        );
    }

    /// Pin the `Display` echo contract for [`FuseUmountError`]. The
    /// impl at `:152-160` collapses every variant into a verbatim
    /// echo of the inner string — clients building error envelopes
    /// rely on this exact passthrough (the `into_cli_error()` call
    /// at `:148` uses `self.to_string()` as the CliError message
    /// body). A future refactor that prefixed variants with
    /// "io read: " / "io write: " would change the user-visible
    /// stderr and break automation that greps the raw inner string.
    #[test]
    fn fuse_umount_error_display_echoes_inner_string_verbatim() {
        assert_eq!(
            FuseUmountError::InvalidTarget("/not/a/path".to_string()).to_string(),
            "/not/a/path",
        );
        assert_eq!(
            FuseUmountError::IoRead("permission denied reading state".to_string()).to_string(),
            "permission denied reading state",
        );
        assert_eq!(
            FuseUmountError::IoWrite("disk full while persisting state".to_string()).to_string(),
            "disk full while persisting state",
        );
    }
}
