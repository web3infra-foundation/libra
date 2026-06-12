//! FUSE mount helpers shared by worktree commands and AI task workspaces.

use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
};

use super::util;

const FUSE_TASK_WORKTREE_PREFIX: &str = "libra-task-worktree-fuse-";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuseTaskWorktreeSweepReport {
    pub scanned: usize,
    pub cleaned: usize,
    pub skipped_live_owner: usize,
    pub failures: Vec<FuseTaskWorktreeSweepFailure>,
}

impl FuseTaskWorktreeSweepReport {
    pub fn is_empty(&self) -> bool {
        self.scanned == 0
            && self.cleaned == 0
            && self.skipped_live_owner == 0
            && self.failures.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FuseTaskWorktreeSweepFailure {
    pub path: PathBuf,
    pub message: String,
}

pub fn normalize_abs_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Path::new(comp.as_os_str())),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

pub fn is_mount_active(mountpoint: &Path) -> bool {
    let target = normalize_abs_path(mountpoint);

    #[cfg(target_os = "linux")]
    {
        mountinfo_contains_mountpoint(&target)
    }

    #[cfg(not(target_os = "linux"))]
    {
        mount_command_contains_mountpoint(&target)
    }
}

pub fn unmount_path(path: &Path) -> io::Result<()> {
    unmount_path_inner(path, false)
}

pub fn force_unmount_path(path: &Path) -> io::Result<()> {
    unmount_path_inner(path, true)
}

fn unmount_path_inner(path: &Path, force: bool) -> io::Result<()> {
    let target = normalize_abs_path(path);
    if !force && !is_mount_active(&target) {
        return Ok(());
    }

    let mut errors = Vec::new();
    for (program, args) in unmount_commands(&target) {
        match Command::new(program).args(&args).output() {
            Ok(output) if output.status.success() => {
                if !is_mount_active(&target) {
                    return Ok(());
                }
                errors.push(format!(
                    "{} succeeded but {} is still mounted",
                    format_command(program, &args),
                    target.display()
                ));
            }
            Ok(output) => {
                errors.push(format!(
                    "{} exited with status {}{}",
                    format_command(program, &args),
                    output.status,
                    command_output_suffix(&output.stderr)
                ));
            }
            Err(err) => {
                errors.push(format!(
                    "{} failed to start: {}",
                    format_command(program, &args),
                    err
                ));
            }
        }
    }

    if !is_mount_active(&target) {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "failed to unmount FUSE path {}: {}",
        target.display(),
        errors.join("; ")
    )))
}

pub fn resolve_task_worktree_mountpoint_arg(path: &Path) -> PathBuf {
    if path.file_name().and_then(|name| name.to_str()) == Some("workspace") {
        return path.to_path_buf();
    }

    let workspace = path.join("workspace");
    if is_fuse_task_worktree_cleanup_root(path) && workspace.exists() {
        workspace
    } else {
        path.to_path_buf()
    }
}

pub fn fuse_task_worktree_cleanup_root(mountpoint: &Path) -> Option<PathBuf> {
    if mountpoint.file_name().and_then(|name| name.to_str()) != Some("workspace") {
        return None;
    }
    let cleanup_root = mountpoint.parent()?;
    if is_fuse_task_worktree_cleanup_root(cleanup_root) {
        Some(cleanup_root.to_path_buf())
    } else {
        None
    }
}

fn is_fuse_task_worktree_cleanup_root(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(FUSE_TASK_WORKTREE_PREFIX))
}

/// Returns the FUSE task-worktree cleanup root that contains `path`, if any.
///
/// Only matches when `path` is at or below `<cleanup_root>/workspace` — the
/// FUSE mount point. Other siblings of the cleanup root (`upper/`, `lower/`,
/// the cleanup root itself) and unrelated nested directories whose basenames
/// happen to share the FUSE prefix are excluded.
///
/// Symlinked paths are resolved via `fs::canonicalize` when possible so that
/// callers passing `~/.../workspace` indirections still match. When
/// canonicalization fails (e.g. the path does not exist on disk, as in unit
/// tests with synthetic fixtures), we fall back to lexical normalization.
pub fn enclosing_fuse_task_worktree_root(path: &Path) -> Option<PathBuf> {
    let normalized = fs::canonicalize(path).unwrap_or_else(|_| normalize_abs_path(path));
    for ancestor in normalized.ancestors() {
        if ancestor.file_name().and_then(|name| name.to_str()) != Some("workspace") {
            continue;
        }
        if let Some(parent) = ancestor.parent()
            && is_fuse_task_worktree_cleanup_root(parent)
        {
            return Some(parent.to_path_buf());
        }
    }
    None
}

/// Returns a stable, FUSE-external cargo target directory for builds rooted at
/// `path` if and only if `path` is inside a FUSE task worktree.
///
/// The libfuse-fs overlay used to back task worktrees rejects some directory
/// creation operations with `EPERM`, which breaks `cargo` (it cannot create
/// `./target` inside the workspace). Redirecting `CARGO_TARGET_DIR` outside the
/// FUSE mount lets builds proceed without changing the workspace contents.
///
/// The path is deterministic per worktree so successive builds reuse the same
/// incremental cache, and the worktree id keeps separate task worktrees from
/// stomping on each other.
pub fn fuse_workspace_cargo_target_dir(path: &Path) -> Option<PathBuf> {
    let cleanup_root = enclosing_fuse_task_worktree_root(path)?;
    let worktree_id = cleanup_root.file_name()?.to_string_lossy().into_owned();
    let mut dir = std::env::temp_dir();
    dir.push("libra-fuse-cargo-target");
    dir.push(worktree_id);
    Some(dir)
}

pub fn sweep_repo_fuse_task_worktrees(
    repo_working_dir: &Path,
) -> io::Result<FuseTaskWorktreeSweepReport> {
    let storage_path = util::try_get_storage_path(Some(repo_working_dir.to_path_buf()))?;
    sweep_fuse_task_worktrees_dir(&storage_path.join("worktrees").join("tasks"))
}

pub fn sweep_fuse_task_worktrees_dir(tasks_dir: &Path) -> io::Result<FuseTaskWorktreeSweepReport> {
    let mut report = FuseTaskWorktreeSweepReport::default();
    if !tasks_dir.exists() {
        return Ok(report);
    }

    for entry in fs::read_dir(tasks_dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                report.failures.push(FuseTaskWorktreeSweepFailure {
                    path: tasks_dir.to_path_buf(),
                    message: err.to_string(),
                });
                continue;
            }
        };
        let root = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) => {
                report.failures.push(FuseTaskWorktreeSweepFailure {
                    path: root,
                    message: err.to_string(),
                });
                continue;
            }
        };
        if !file_type.is_dir() || !is_fuse_task_worktree_cleanup_root(&root) {
            continue;
        }

        report.scanned += 1;
        if let Some(pid) = task_worktree_owner_pid(&root)
            && pid != std::process::id()
            && pid_is_live(pid)
        {
            report.skipped_live_owner += 1;
            continue;
        }

        if let Err(err) = unmount_and_remove_fuse_task_worktree(&root) {
            report.failures.push(FuseTaskWorktreeSweepFailure {
                path: root,
                message: err.to_string(),
            });
        } else {
            report.cleaned += 1;
        }
    }

    Ok(report)
}

fn unmount_and_remove_fuse_task_worktree(root: &Path) -> io::Result<()> {
    let workspace = root.join("workspace");
    if workspace.exists() || is_mount_active(&workspace) {
        force_unmount_path(&workspace)?;
    }
    fs::remove_dir_all(root)
}

fn task_worktree_owner_pid(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let suffix = name.strip_prefix(FUSE_TASK_WORKTREE_PREFIX)?;
    let pid = suffix.split('-').next()?;
    pid.parse().ok()
}

#[cfg(unix)]
fn pid_is_live(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    // SAFETY: kill(pid, 0) performs a liveness probe without sending a signal.
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if result == 0 {
        return true;
    }

    match io::Error::last_os_error().raw_os_error() {
        Some(code) if code == libc::ESRCH => false,
        Some(code) if code == libc::EPERM => true,
        _ => false,
    }
}

#[cfg(not(unix))]
fn pid_is_live(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn mountinfo_contains_mountpoint(target: &Path) -> bool {
    let Ok(content) = fs::read_to_string("/proc/self/mountinfo") else {
        return mount_command_contains_mountpoint(target);
    };
    let target = target.to_string_lossy();
    content.lines().any(|line| {
        let mut parts = line.split_whitespace();
        let _ = parts.next();
        let _ = parts.next();
        let _ = parts.next();
        let _ = parts.next();
        match parts.next() {
            Some(mountpoint) => decode_mount_escaped(mountpoint) == target,
            None => false,
        }
    })
}

fn mount_command_contains_mountpoint(target: &Path) -> bool {
    let Ok(output) = Command::new("mount").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let target = target.to_string_lossy();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(mount_command_mountpoint)
        .any(|mountpoint| normalize_abs_path(Path::new(mountpoint)).to_string_lossy() == target)
}

fn mount_command_mountpoint(line: &str) -> Option<&str> {
    let (_, after_on) = line.split_once(" on ")?;
    let (mountpoint, _) = after_on.rsplit_once(" (")?;
    Some(mountpoint)
}

#[cfg(any(target_os = "linux", test))]
fn decode_mount_escaped(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn unmount_commands(target: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let target = target.as_os_str().to_os_string();
    let mut commands = Vec::new();

    #[cfg(target_os = "linux")]
    {
        commands.push(("fusermount3", vec![OsString::from("-u"), target.clone()]));
        commands.push(("fusermount", vec![OsString::from("-u"), target.clone()]));
    }

    #[cfg(target_os = "macos")]
    {
        commands.push(("/sbin/umount", vec![target.clone()]));
    }

    commands.push(("umount", vec![target]));
    commands
}

fn format_command(program: &str, args: &[OsString]) -> String {
    let mut rendered = program.to_string();
    for arg in args {
        rendered.push(' ');
        rendered.push_str(&arg.to_string_lossy());
    }
    rendered
}

fn command_output_suffix(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_mount_command_mountpoint_with_spaces() {
        let line = "macfuse0 on /Volumes/Data/linked path/workspace (macfuse, local)";

        assert_eq!(
            mount_command_mountpoint(line),
            Some("/Volumes/Data/linked path/workspace")
        );
    }

    #[test]
    fn decodes_linux_mountinfo_escapes() {
        assert_eq!(
            decode_mount_escaped("/tmp/libra\\040task\\011workspace\\134x"),
            "/tmp/libra task\tworkspace\\x"
        );
    }

    #[test]
    fn resolves_task_worktree_mountpoint_from_cleanup_root() {
        let temp = tempdir().expect("create temp dir");
        let root = temp
            .path()
            .join("libra-task-worktree-fuse-123-019ddec6-de60-7383");
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("create workspace");

        assert_eq!(resolve_task_worktree_mountpoint_arg(&root), workspace);
    }

    #[test]
    fn keeps_workspace_mountpoint_arg_unchanged() {
        let temp = tempdir().expect("create temp dir");
        let workspace = temp
            .path()
            .join("libra-task-worktree-fuse-123-019ddec6-de60-7383")
            .join("workspace");

        assert_eq!(resolve_task_worktree_mountpoint_arg(&workspace), workspace);
    }

    #[test]
    fn detects_fuse_task_worktree_cleanup_root_from_workspace() {
        let mountpoint =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-29353-id/workspace");

        assert_eq!(
            fuse_task_worktree_cleanup_root(mountpoint),
            Some(PathBuf::from(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-29353-id"
            ))
        );
        assert_eq!(
            fuse_task_worktree_cleanup_root(Path::new("/tmp/workspace")),
            None
        );
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_walks_to_cleanup_dir() {
        let cwd = Path::new(
            "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace/src/main.rs",
        );
        assert_eq!(
            enclosing_fuse_task_worktree_root(cwd),
            Some(PathBuf::from(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d"
            ))
        );
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_matches_workspace_dir_itself() {
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace");
        assert_eq!(
            enclosing_fuse_task_worktree_root(cwd),
            Some(PathBuf::from(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d"
            ))
        );
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_ignores_cleanup_root_itself() {
        // Only `<cleanup_root>/workspace[/...]` is the FUSE mount point.
        // Passing the cleanup root directly is not "inside the worktree" for
        // build purposes — that path holds `lower/`, `upper/` and the
        // unmount-time scaffolding rather than the user's cwd.
        let cwd = Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d");
        assert_eq!(enclosing_fuse_task_worktree_root(cwd), None);
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_ignores_overlay_layers() {
        for layer in ["upper", "lower"] {
            let cwd = PathBuf::from(format!(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/{layer}/main.rs"
            ));
            assert_eq!(
                enclosing_fuse_task_worktree_root(&cwd),
                None,
                "layer {layer} must not be treated as the FUSE mount point"
            );
        }
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_picks_outer_when_workspace_contains_prefix_dir() {
        // A workspace might happen to contain a directory whose basename
        // re-uses the FUSE prefix (the user could check it in by accident).
        // The detection must still resolve to the *outer* worktree's cleanup
        // root, not the inner basename, because only the outer path is the
        // real FUSE mount.
        let cwd = Path::new(
            "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace/foo/libra-task-worktree-fuse-99-zz/bar.rs",
        );
        assert_eq!(
            enclosing_fuse_task_worktree_root(cwd),
            Some(PathBuf::from(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d"
            ))
        );
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_ignores_copy_backend() {
        let cwd = Path::new(
            "/repo/.libra/worktrees/tasks/libra-task-worktree-copy-7-019d/workspace/src/main.rs",
        );
        assert_eq!(enclosing_fuse_task_worktree_root(cwd), None);
    }

    #[test]
    fn enclosing_fuse_task_worktree_root_returns_none_outside_worktree() {
        assert_eq!(
            enclosing_fuse_task_worktree_root(Path::new("/repo/src/main.rs")),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn enclosing_fuse_task_worktree_root_resolves_symlinked_workspace() {
        // Real worktrees on macOS often live under `/var/folders/...` while a
        // friendlier symlink may be advertised via `/tmp` (which itself is a
        // symlink to `/private/tmp`). Canonicalisation must still reach the
        // FUSE prefix in the resolved path.
        let temp = tempdir().expect("create temp dir");
        let cleanup_root = temp.path().join("libra-task-worktree-fuse-13-019sym");
        let workspace = cleanup_root.join("workspace");
        fs::create_dir_all(workspace.join("src")).expect("create workspace tree");
        let symlink = temp.path().join("symlinked-cwd");
        std::os::unix::fs::symlink(&workspace, &symlink).expect("create symlink");

        let resolved =
            enclosing_fuse_task_worktree_root(&symlink).expect("symlinked cwd must resolve");
        let expected = fs::canonicalize(&cleanup_root).unwrap_or(cleanup_root);
        assert_eq!(resolved, expected);
    }

    #[test]
    fn fuse_workspace_cargo_target_dir_returns_stable_per_worktree_path() {
        let cwd =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace/src");
        let dir = fuse_workspace_cargo_target_dir(cwd).expect("FUSE worktree must yield a dir");
        let expected = std::env::temp_dir()
            .join("libra-fuse-cargo-target")
            .join("libra-task-worktree-fuse-7-019d");
        assert_eq!(dir, expected);

        // Same worktree → same dir, regardless of subpath depth (incremental
        // builds rely on this).
        let nested = Path::new(
            "/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-7-019d/workspace/deep/nest/x.rs",
        );
        assert_eq!(fuse_workspace_cargo_target_dir(nested), Some(expected));
    }

    #[test]
    fn fuse_workspace_cargo_target_dir_returns_none_outside_fuse_worktree() {
        assert_eq!(
            fuse_workspace_cargo_target_dir(Path::new("/repo/src/main.rs")),
            None
        );
    }

    #[test]
    fn parses_task_worktree_owner_pid() {
        let root =
            Path::new("/repo/.libra/worktrees/tasks/libra-task-worktree-fuse-29353-019ddec6-id");

        assert_eq!(task_worktree_owner_pid(root), Some(29353));
        assert_eq!(
            task_worktree_owner_pid(Path::new(
                "/repo/.libra/worktrees/tasks/libra-task-worktree-copy-29353-id"
            )),
            None
        );
    }

    #[test]
    fn sweep_fuse_task_worktrees_dir_allows_missing_directory() {
        let temp = tempdir().expect("create temp dir");
        let report =
            sweep_fuse_task_worktrees_dir(&temp.path().join("missing")).expect("sweep succeeds");

        assert!(report.is_empty());
    }

    #[test]
    fn sweep_fuse_task_worktrees_dir_cleans_stale_fuse_roots() {
        let temp = tempdir().expect("create temp dir");
        let tasks_dir = temp.path().join("tasks");
        let root = tasks_dir.join("libra-task-worktree-fuse-999999-019ddec6-de60-7383");
        fs::create_dir_all(root.join("workspace")).expect("create workspace");

        let report = sweep_fuse_task_worktrees_dir(&tasks_dir).expect("sweep succeeds");

        assert_eq!(report.scanned, 1);
        assert_eq!(report.cleaned, 1);
        assert_eq!(report.skipped_live_owner, 0);
        assert!(report.failures.is_empty());
        assert!(!root.exists());
    }

    #[test]
    fn sweep_fuse_task_worktrees_dir_ignores_non_fuse_roots() {
        let temp = tempdir().expect("create temp dir");
        let tasks_dir = temp.path().join("tasks");
        let copy_root = tasks_dir.join("libra-task-worktree-copy-999999-019ddec6-de60-7383");
        let regular_root = tasks_dir.join("regular");
        fs::create_dir_all(copy_root.join("workspace")).expect("create copy workspace");
        fs::create_dir_all(&regular_root).expect("create regular root");

        let report = sweep_fuse_task_worktrees_dir(&tasks_dir).expect("sweep succeeds");

        assert!(report.is_empty());
        assert!(copy_root.exists());
        assert!(regular_root.exists());
    }

    #[test]
    fn sweep_fuse_task_worktrees_dir_skips_live_foreign_owner() {
        let temp = tempdir().expect("create temp dir");
        let tasks_dir = temp.path().join("tasks");
        let current_pid = std::process::id();
        let live_foreign_pid = if current_pid == 1 { 2 } else { 1 };
        let root = tasks_dir.join(format!(
            "libra-task-worktree-fuse-{live_foreign_pid}-019ddec6-de60-7383"
        ));
        fs::create_dir_all(root.join("workspace")).expect("create workspace");

        let report = sweep_fuse_task_worktrees_dir(&tasks_dir).expect("sweep succeeds");

        if pid_is_live(live_foreign_pid) {
            assert_eq!(report.scanned, 1);
            assert_eq!(report.cleaned, 0);
            assert_eq!(report.skipped_live_owner, 1);
            assert!(root.exists());
        } else {
            assert_eq!(report.cleaned, 1);
            assert!(!root.exists());
        }
    }
}
