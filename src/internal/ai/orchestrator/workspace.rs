use std::{
    fs, io,
    path::{Path, PathBuf},
};

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
}

pub(crate) fn prepare_task_worktree(
    main_working_dir: &Path,
    task_id: Uuid,
) -> io::Result<TaskWorktree> {
    let root = std::env::temp_dir().join(format!(
        "libra-task-worktree-{}-{}",
        std::process::id(),
        task_id
    ));

    if root.exists() {
        fs::remove_dir_all(&root)?;
    }
    fs::create_dir_all(&root)?;
    match util::try_get_storage_path(Some(main_working_dir.to_path_buf())) {
        Ok(storage) => create_storage_link(&storage, &root.join(util::ROOT_DIR))?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

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
        cleanup_task_worktree, clone_or_copy_file, materialize_workspace, prepare_task_worktree,
        sync_task_worktree_back,
    };
    use crate::{
        internal::ai::workspace_snapshot::{WorkspaceEntry, snapshot_workspace},
        utils::util,
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

        let task_worktree = prepare_task_worktree(&main, Uuid::new_v4()).unwrap();

        assert_eq!(
            std::fs::read_to_string(task_worktree.root.join("src/lib.rs")).unwrap(),
            "fn main() {}\n"
        );
        assert!(!task_worktree.root.join(util::ROOT_DIR).exists());

        cleanup_task_worktree(&task_worktree.root).unwrap();
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

        cleanup_task_worktree(&task_worktree.root).unwrap();
    }
}
