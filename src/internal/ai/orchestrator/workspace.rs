#[cfg(unix)]
use std::sync::Arc;
use std::{
    fs, io,
    path::{Path, PathBuf},
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

use super::acl::{ScopeVerdict, check_scope};
use crate::{
    internal::ai::workspace_snapshot::{
        WorkspaceSnapshot, changed_paths_since_baseline, snapshot_workspace,
        workspace_entry_if_exists,
    },
    utils::util,
};

pub(crate) struct TaskWorktree {
    pub(crate) root: PathBuf,
    pub(crate) baseline: WorkspaceSnapshot,
    backend: TaskWorktreeBackend,
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
    #[cfg(unix)]
    lower_root: PathBuf,
    #[cfg(unix)]
    upper_root: PathBuf,
}

pub(crate) fn prepare_task_worktree(
    main_working_dir: &Path,
    task_id: Uuid,
) -> io::Result<TaskWorktree> {
    let baseline = snapshot_workspace(main_working_dir)?;

    #[cfg(unix)]
    {
        let fuse_paths = task_worktree_paths(task_id, "fuse");
        if let Some(backend) = prepare_fuse_task_worktree(main_working_dir, &fuse_paths, &baseline)?
        {
            return Ok(TaskWorktree {
                root: fuse_paths.workspace_root,
                baseline,
                backend,
            });
        }
    }

    let copy_paths = task_worktree_paths(task_id, "copy");
    prepare_task_worktree_root(&copy_paths.cleanup_root)?;
    let backend = prepare_copy_task_worktree(main_working_dir, &copy_paths, &baseline)?;

    Ok(TaskWorktree {
        root: copy_paths.workspace_root,
        baseline,
        backend,
    })
}

fn task_worktree_paths(task_id: Uuid, backend: &str) -> TaskWorktreePaths {
    let cleanup_root = std::env::temp_dir().join(format!(
        "libra-task-worktree-{}-{}-{}",
        backend,
        std::process::id(),
        task_id
    ));
    TaskWorktreePaths {
        workspace_root: cleanup_root.join("workspace"),
        #[cfg(unix)]
        lower_root: cleanup_root.join("lower"),
        #[cfg(unix)]
        upper_root: cleanup_root.join("upper"),
        cleanup_root,
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
        Ok(storage) => populate_repo_storage(
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
) -> io::Result<Option<TaskWorktreeBackend>> {
    let Ok(runtime) = Handle::try_current() else {
        return Ok(None);
    };

    prepare_task_worktree_root(&paths.cleanup_root)?;
    fs::create_dir_all(&paths.workspace_root)?;
    fs::create_dir_all(&paths.lower_root)?;
    fs::create_dir_all(&paths.upper_root)?;
    materialize_workspace(main_working_dir, &paths.lower_root, baseline)?;

    match util::try_get_storage_path(Some(main_working_dir.to_path_buf())) {
        Ok(storage) => {
            populate_repo_storage(
                &storage,
                &paths.upper_root.join(util::ROOT_DIR),
                "FUSE upper layer",
            )?;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let mount_result = runtime.block_on(mount_fuse_task_worktree(
        &paths.lower_root,
        &paths.workspace_root,
        &paths.upper_root,
    ));

    match mount_result {
        Ok(mount_handle) => {
            if let Err(err) = verify_fuse_task_worktree_mount(&paths.workspace_root) {
                if let Err(unmount_err) = runtime.block_on(mount_handle.unmount()) {
                    warn!(
                        mount = %paths.workspace_root.display(),
                        "failed to unmount unhealthy FUSE task worktree before fallback: {}",
                        unmount_err
                    );
                }
                warn_cleanup_root_failure(&paths.cleanup_root);
                warn!(
                    path = %main_working_dir.display(),
                    mount = %paths.workspace_root.display(),
                    "mounted FUSE task worktree failed health check, falling back to copy backend: {}",
                    err
                );
                return Ok(None);
            }

            Ok(Some(TaskWorktreeBackend::Fuse(FuseTaskWorktreeBackend {
                cleanup_root: paths.cleanup_root.clone(),
                mount_handle,
            })))
        }
        Err(err) => {
            warn_cleanup_root_failure(&paths.cleanup_root);
            warn!(
                path = %main_working_dir.display(),
                mount = %paths.workspace_root.display(),
                "failed to mount FUSE task worktree, falling back to copy backend: {}",
                err
            );
            Ok(None)
        }
    }
}

#[cfg(unix)]
fn verify_fuse_task_worktree_mount(workspace_root: &Path) -> io::Result<()> {
    fs::read_dir(workspace_root)?;
    fs::symlink_metadata(workspace_root.join(util::ROOT_DIR))?;
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
        .await?,
    );
    let upper_layer = Arc::new(
        new_passthroughfs_layer(PassthroughArgs {
            root_dir: upper_root,
            mapping: None::<&str>,
        })
        .await?,
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
    )?;

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
) -> io::Result<()> {
    let task_snapshot = snapshot_workspace(task_worktree_dir)?;
    let changed_paths = changed_paths_since_baseline(baseline, &task_snapshot);

    for rel_path in &changed_paths {
        let rel_path_str = rel_path.to_string_lossy();
        if let Some(reason) =
            sync_contract_violation(touch_files, in_scope, out_of_scope, &rel_path_str)
        {
            return Err(io::Error::other(format!(
                "task worktree modified '{}' outside its declared contract: {}",
                rel_path.display(),
                reason
            )));
        }
    }

    for rel_path in &changed_paths {
        let expected = baseline.entries.get(rel_path).cloned();
        let actual = workspace_entry_if_exists(&main_working_dir.join(rel_path))?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "main workspace changed concurrently at '{}'",
                rel_path.display()
            )));
        }
    }

    for rel_path in changed_paths {
        if task_snapshot.entries.contains_key(&rel_path) {
            copy_workspace_entry(task_worktree_dir, main_working_dir, &rel_path)?;
        } else {
            remove_workspace_entry(main_working_dir, &rel_path)?;
        }
    }

    Ok(())
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
        return match check_scope(touch_files, &[], path) {
            ScopeVerdict::InScope => None,
            ScopeVerdict::OutOfScope(reason) => Some(format!("not in touchFiles: {reason}")),
        };
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

#[cfg(windows)]
fn is_windows_symlink_privilege_error(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(1314)
}

fn populate_repo_storage(storage: &Path, target_path: &Path, context: &str) -> io::Result<()> {
    populate_repo_storage_with_link(storage, target_path, context, create_storage_link)
}

fn populate_repo_storage_with_link(
    storage: &Path,
    target_path: &Path,
    context: &str,
    link: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    match link(storage, target_path) {
        Ok(()) => Ok(()),
        #[cfg(windows)]
        Err(err) if is_windows_symlink_privilege_error(&err) => {
            copy_dir_all(storage, target_path).map_err(|copy_err| {
                io::Error::new(
                    copy_err.kind(),
                    format!(
                        "failed to populate repository storage '{}' into {} at '{}' after symlink fallback: {}",
                        storage.display(),
                        context,
                        target_path.display(),
                        copy_err
                    ),
                )
            })
        }
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!(
                "failed to link repository storage '{}' into {} at '{}': {}",
                storage.display(),
                context,
                target_path.display(),
                err
            ),
        )),
    }
}

#[cfg(any(windows, test))]
fn copy_dir_all(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&source_path, &target_path)?;
        } else if file_type.is_symlink() {
            let link_target = fs::read_link(&source_path)?;
            create_symlink(&link_target, &source_path, &target_path)?;
        } else {
            clone_or_copy_file(&source_path, &target_path)?;
        }
    }
    Ok(())
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

    #[cfg(windows)]
    use super::populate_repo_storage_with_link;
    use super::{
        cleanup_task_worktree, copy_dir_all, materialize_workspace, prepare_task_worktree,
        sync_task_worktree_back,
    };
    use crate::{
        internal::ai::workspace_snapshot::{WorkspaceEntry, snapshot_workspace},
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

    #[cfg(windows)]
    fn is_windows_symlink_privilege_error(err: &io::Error) -> bool {
        err.kind() == io::ErrorKind::PermissionDenied || err.raw_os_error() == Some(1314)
    }

    #[test]
    fn copy_dir_all_copies_nested_storage_tree() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        std::fs::create_dir_all(source.join("objects/ab")).unwrap();
        std::fs::write(source.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(source.join("objects/ab/cd"), "blob\n").unwrap();

        copy_dir_all(&source, &target).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("objects/ab/cd")).unwrap(),
            "blob\n"
        );
    }

    #[test]
    #[cfg(windows)]
    fn populate_repo_storage_falls_back_to_copy_on_windows_symlink_privilege_error() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("storage");
        let target = temp.path().join("workspace/.libra");
        std::fs::create_dir_all(source.join("objects/ab")).unwrap();
        std::fs::write(source.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(source.join("objects/ab/cd"), "blob\n").unwrap();

        let err = io::Error::from_raw_os_error(1314);
        populate_repo_storage_with_link(&source, &target, "copy task worktree", |_, _| Err(err))
            .unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("HEAD")).unwrap(),
            "ref: refs/heads/main\n"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("objects/ab/cd")).unwrap(),
            "blob\n"
        );
    }

    #[test]
    fn snapshot_records_directory_symlink_without_recursing() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        let external = temp.path().join("external");
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        std::fs::write(external.join("secret.txt"), "outside\n").unwrap();
        if let Err(err) = symlink_path(&external, &root.join("nested").join("external-link")) {
            #[cfg(windows)]
            if is_windows_symlink_privilege_error(&err) {
                eprintln!("skipping directory symlink test on Windows without symlink privilege");
                return;
            }
            panic!("failed to create directory symlink fixture: {err}");
        }

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
        if let Err(err) = symlink_path(std::path::Path::new("target.txt"), &main.join("link.txt")) {
            #[cfg(windows)]
            if is_windows_symlink_privilege_error(&err) {
                eprintln!(
                    "skipping symlink preservation test on Windows without symlink privilege"
                );
                return;
            }
            panic!("failed to create source symlink fixture: {err}");
        }

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
        if let Err(err) = symlink_path(std::path::Path::new("updated.txt"), &task.join("link.txt"))
        {
            #[cfg(windows)]
            if is_windows_symlink_privilege_error(&err) {
                eprintln!(
                    "skipping symlink preservation test on Windows without symlink privilege"
                );
                return;
            }
            panic!("failed to update task symlink fixture: {err}");
        }

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

        assert!(err.to_string().contains("outside its declared contract"));
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

        let task_worktree = prepare_task_worktree(&main, Uuid::new_v4()).unwrap();

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

        let task_worktree = prepare_task_worktree(&main, Uuid::new_v4()).unwrap();

        assert!(task_worktree.root.join("src/lib.rs").exists());
        assert!(!task_worktree.root.join("target").exists());

        cleanup_task_worktree(task_worktree).unwrap();
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
        let task_worktree = tokio::task::spawn_blocking(move || {
            prepare_task_worktree(&repo_for_prepare, Uuid::new_v4())
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
