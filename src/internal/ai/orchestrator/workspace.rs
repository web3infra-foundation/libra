use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use git_internal::hash::ObjectHash;
use uuid::Uuid;

use crate::{command::calc_file_blob_hash, utils::util};

#[derive(Clone, Debug, Default)]
pub(crate) struct WorkspaceSnapshot {
    pub(crate) files: BTreeMap<PathBuf, ObjectHash>,
}

pub(crate) struct TaskWorktree {
    pub(crate) root: PathBuf,
    pub(crate) baseline: WorkspaceSnapshot,
}

pub(crate) fn prepare_task_worktree(
    main_working_dir: &Path,
    task_id: Uuid,
) -> io::Result<TaskWorktree> {
    let storage = util::try_get_storage_path(Some(main_working_dir.to_path_buf()))?;
    let root = std::env::temp_dir().join(format!(
        "libra-task-worktree-{}-{}",
        std::process::id(),
        task_id
    ));

    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    create_storage_link(&storage, &root.join(util::ROOT_DIR))?;

    let baseline = snapshot_workspace(main_working_dir)?;
    materialize_workspace(main_working_dir, &root, &baseline)?;

    Ok(TaskWorktree { root, baseline })
}

pub(crate) fn cleanup_task_worktree(root: &Path) -> io::Result<()> {
    if root.exists() {
        fs::remove_dir_all(root)?;
    }
    Ok(())
}

pub(crate) fn sync_task_worktree_back(
    main_working_dir: &Path,
    task_worktree_dir: &Path,
    baseline: &WorkspaceSnapshot,
) -> io::Result<()> {
    let task_snapshot = snapshot_workspace(task_worktree_dir)?;
    let changed_paths = changed_paths_since_baseline(baseline, &task_snapshot);

    for rel_path in &changed_paths {
        let expected = baseline.files.get(rel_path).copied();
        let actual = file_hash_if_exists(&main_working_dir.join(rel_path))?;
        if actual != expected {
            return Err(io::Error::other(format!(
                "main workspace changed concurrently at '{}'",
                rel_path.display()
            )));
        }
    }

    for rel_path in changed_paths {
        if task_snapshot.files.contains_key(&rel_path) {
            copy_workspace_file(task_worktree_dir, main_working_dir, &rel_path)?;
        } else {
            remove_workspace_file(main_working_dir, &rel_path)?;
        }
    }

    Ok(())
}

fn snapshot_workspace(root: &Path) -> io::Result<WorkspaceSnapshot> {
    fn visit_dir(
        root: &Path,
        dir: &Path,
        files: &mut BTreeMap<PathBuf, ObjectHash>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_name() == util::ROOT_DIR {
                continue;
            }

            let path = entry.path();
            let metadata = fs::metadata(&path)?;
            if metadata.is_dir() {
                visit_dir(root, &path, files)?;
                continue;
            }

            let rel = path
                .strip_prefix(root)
                .map_err(|err| io::Error::other(err.to_string()))?
                .to_path_buf();
            files.insert(rel, calc_file_blob_hash(&path)?);
        }
        Ok(())
    }

    let mut files = BTreeMap::new();
    visit_dir(root, root, &mut files)?;
    Ok(WorkspaceSnapshot { files })
}

fn materialize_workspace(
    source_root: &Path,
    target_root: &Path,
    snapshot: &WorkspaceSnapshot,
) -> io::Result<()> {
    for rel_path in snapshot.files.keys() {
        copy_workspace_file(source_root, target_root, rel_path)?;
    }
    Ok(())
}

fn changed_paths_since_baseline(
    baseline: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
) -> Vec<PathBuf> {
    let paths = baseline
        .files
        .keys()
        .chain(current.files.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    paths
        .into_iter()
        .filter(|path| baseline.files.get(path) != current.files.get(path))
        .collect()
}

fn file_hash_if_exists(path: &Path) -> io::Result<Option<ObjectHash>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(calc_file_blob_hash(path)?))
}

fn copy_workspace_file(source_root: &Path, target_root: &Path, rel_path: &Path) -> io::Result<()> {
    let source = source_root.join(rel_path);
    let target = target_root.join(rel_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    clone_or_copy_file(&source, &target)?;
    let permissions = fs::metadata(&source)?.permissions();
    fs::set_permissions(&target, permissions)?;
    Ok(())
}

fn clone_or_copy_file(source: &Path, target: &Path) -> io::Result<()> {
    if target.exists() {
        fs::remove_file(target)?;
    }

    match try_clone_file_cow(source, target) {
        Ok(()) => Ok(()),
        Err(_) => {
            if target.exists() {
                let _ = fs::remove_file(target);
            }
            fs::copy(source, target)?;
            Ok(())
        }
    }
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

fn remove_workspace_file(root: &Path, rel_path: &Path) -> io::Result<()> {
    let target = root.join(rel_path);
    if target.exists() {
        fs::remove_file(&target)?;
        remove_empty_parents(root, target.parent());
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

#[cfg(windows)]
fn create_storage_link(storage: &Path, link_path: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(storage, link_path)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::clone_or_copy_file;

    #[test]
    fn clone_or_copy_file_preserves_contents() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("source.txt");
        let target = temp.path().join("target.txt");
        std::fs::write(&source, "cow me maybe\n").unwrap();

        clone_or_copy_file(&source, &target).unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "cow me maybe\n");
    }
}
