//! Workspace snapshot utilities used to compare task worktrees against a baseline.
//!
//! Boundary: snapshots record relative paths, file content hashes, metadata kind, and
//! deletion state without following symlinks outside the workspace. Orchestrator
//! workspace tests cover symlink, deletion, and changed-file edge cases.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

use git_internal::{hash::ObjectHash, internal::object::blob::Blob};
use ignore::WalkBuilder;

use crate::{internal::ai::generated_artifacts, utils::object_ext::BlobExt};

#[derive(Clone, Debug, Default)]
pub(crate) struct WorkspaceSnapshot {
    pub(crate) entries: BTreeMap<PathBuf, WorkspaceEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceEntry {
    File(ObjectHash),
    Symlink(PathBuf),
}

pub(crate) fn snapshot_workspace(root: &Path) -> io::Result<WorkspaceSnapshot> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_path(|left, right| left.cmp(right))
        .filter_entry(|entry| {
            let is_dir = entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir());
            entry.depth() == 0 || !ignored_workspace_entry(entry.path(), is_dir)
        });

    let mut entries = BTreeMap::new();
    for entry in builder.build() {
        let entry = entry.map_err(ignore_error_to_io)?;
        let path = entry.path();
        if path == root {
            continue;
        }

        let file_type = if let Some(file_type) = entry.file_type() {
            file_type
        } else {
            fs::symlink_metadata(path)?.file_type()
        };
        if file_type.is_dir() {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .map_err(|err| io::Error::other(err.to_string()))?
            .to_path_buf();
        entries.insert(rel, snapshot_entry(path, &file_type)?);
    }

    Ok(WorkspaceSnapshot { entries })
}

pub(crate) fn changed_paths_since_baseline(
    baseline: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
) -> Vec<PathBuf> {
    let paths = baseline
        .entries
        .keys()
        .chain(current.entries.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    paths
        .into_iter()
        .filter(|path| baseline.entries.get(path) != current.entries.get(path))
        .collect()
}

pub(crate) fn workspace_entry_if_exists(path: &Path) -> io::Result<Option<WorkspaceEntry>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => snapshot_entry(path, &metadata.file_type()).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn ignored_workspace_entry(path: &Path, is_dir: bool) -> bool {
    protected_workspace_entry(path)
        || (is_dir && generated_artifacts::is_generated_build_dir_path(path))
}

fn protected_workspace_entry(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".libra" | ".codex" | ".agents"))
}

fn snapshot_entry(path: &Path, file_type: &fs::FileType) -> io::Result<WorkspaceEntry> {
    if file_type.is_symlink() {
        return Ok(WorkspaceEntry::Symlink(fs::read_link(path)?));
    }

    // Workspace snapshots are used only for change detection between two local
    // filesystem states. They should not depend on repository-scoped LFS or
    // attribute resolution, because isolated task workspaces and tests may run
    // outside a Libra repository context.
    Ok(WorkspaceEntry::File(Blob::from_file(path).id))
}

fn ignore_error_to_io(err: ignore::Error) -> io::Error {
    let err_text = err.to_string();
    err.into_io_error()
        .unwrap_or_else(|| io::Error::other(err_text))
}

#[cfg(test)]
mod tests {
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

    use tempfile::tempdir;

    use super::{WorkspaceEntry, snapshot_workspace};

    #[cfg(unix)]
    fn symlink_path(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn symlink_path(target: &Path, link: &Path) -> io::Result<()> {
        match fs::metadata(target) {
            Ok(metadata) if metadata.is_dir() => std::os::windows::fs::symlink_dir(target, link),
            _ => std::os::windows::fs::symlink_file(target, link),
        }
    }
    #[test]
    fn snapshot_respects_gitignore_without_git_dir() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("web/node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::write(root.join(".gitignore"), "target/\nweb/node_modules/\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
        fs::write(root.join("target/debug/app"), "bin\n").unwrap();
        fs::write(root.join("web/node_modules/pkg/index.js"), "export {};\n").unwrap();
        fs::write(root.join(".cargo/config.toml"), "[build]\n").unwrap();

        let snapshot = snapshot_workspace(&root).unwrap();

        assert!(snapshot.entries.contains_key(Path::new("src/lib.rs")));
        assert!(
            snapshot
                .entries
                .contains_key(Path::new(".cargo/config.toml"))
        );
        assert!(!snapshot.entries.contains_key(Path::new("target/debug/app")));
        assert!(
            !snapshot
                .entries
                .contains_key(Path::new("web/node_modules/pkg/index.js"))
        );
    }

    #[test]
    fn snapshot_skips_default_build_outputs_without_gitignore() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::write(root.join("Cargo.lock"), "# lock\n").unwrap();
        fs::write(root.join("target/.rustc_info.json"), "{}\n").unwrap();
        fs::write(root.join("target/debug/app"), "compiled\n").unwrap();

        let snapshot = snapshot_workspace(&root).unwrap();

        assert!(snapshot.entries.contains_key(Path::new("Cargo.lock")));
        assert!(
            !snapshot
                .entries
                .contains_key(Path::new("target/.rustc_info.json"))
        );
        assert!(!snapshot.entries.contains_key(Path::new("target/debug/app")));
    }

    #[test]
    fn snapshot_skips_common_compiled_language_build_outputs_without_gitignore() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("rust/target/debug")).unwrap();
        fs::create_dir_all(root.join("java/build/classes")).unwrap();
        fs::create_dir_all(root.join("java/target/classes")).unwrap();
        fs::create_dir_all(root.join("dotnet/bin/Debug")).unwrap();
        fs::create_dir_all(root.join("dotnet/obj")).unwrap();
        fs::create_dir_all(root.join("swift/.build/debug")).unwrap();
        fs::create_dir_all(root.join("zig/.zig-cache")).unwrap();
        fs::create_dir_all(root.join("zig/zig-out/bin")).unwrap();
        fs::create_dir_all(root.join("cpp/cmake-build-debug")).unwrap();
        fs::create_dir_all(root.join("cpp/CMakeFiles/app.dir")).unwrap();
        fs::create_dir_all(root.join("bazel-bin")).unwrap();
        fs::create_dir_all(root.join("bazel-out")).unwrap();
        fs::create_dir_all(root.join("bazel-testlogs")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("src/bin")).unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
        fs::write(root.join("src/bin/tool.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("rust/target/debug/app"), "compiled\n").unwrap();
        fs::write(root.join("java/build/classes/App.class"), "compiled\n").unwrap();
        fs::write(root.join("java/target/classes/App.class"), "compiled\n").unwrap();
        fs::write(root.join("dotnet/bin/Debug/app.dll"), "compiled\n").unwrap();
        fs::write(root.join("dotnet/obj/project.assets.json"), "{}\n").unwrap();
        fs::write(root.join("swift/.build/debug/app"), "compiled\n").unwrap();
        fs::write(root.join("zig/.zig-cache/state"), "cache\n").unwrap();
        fs::write(root.join("zig/zig-out/bin/app"), "compiled\n").unwrap();
        fs::write(root.join("cpp/cmake-build-debug/app"), "compiled\n").unwrap();
        fs::write(root.join("cpp/CMakeFiles/app.dir/main.o"), "compiled\n").unwrap();
        fs::write(root.join("bazel-bin/app"), "compiled\n").unwrap();
        fs::write(root.join("bazel-out/state"), "cache\n").unwrap();
        fs::write(root.join("bazel-testlogs/test.log"), "log\n").unwrap();

        let snapshot = snapshot_workspace(&root).unwrap();

        assert!(snapshot.entries.contains_key(Path::new("src/lib.rs")));
        assert!(snapshot.entries.contains_key(Path::new("src/bin/tool.rs")));
        for generated in [
            "rust/target/debug/app",
            "java/build/classes/App.class",
            "java/target/classes/App.class",
            "dotnet/bin/Debug/app.dll",
            "dotnet/obj/project.assets.json",
            "swift/.build/debug/app",
            "zig/.zig-cache/state",
            "zig/zig-out/bin/app",
            "cpp/cmake-build-debug/app",
            "cpp/CMakeFiles/app.dir/main.o",
            "bazel-bin/app",
            "bazel-out/state",
            "bazel-testlogs/test.log",
        ] {
            assert!(
                !snapshot.entries.contains_key(Path::new(generated)),
                "{generated} should be ignored"
            );
        }
    }

    #[test]
    fn snapshot_skips_protected_metadata_dirs_and_keeps_symlinks() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join(".libra")).unwrap();
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::create_dir_all(root.join(".agents")).unwrap();
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join(".libra/db"), "sqlite\n").unwrap();
        fs::write(root.join(".codex/session"), "state\n").unwrap();
        fs::write(root.join(".agents/cache"), "cache\n").unwrap();
        fs::write(root.join("real.txt"), "hello\n").unwrap();
        symlink_path(Path::new("real.txt"), &root.join("nested/link.txt")).unwrap();

        let snapshot = snapshot_workspace(&root).unwrap();

        assert!(!snapshot.entries.contains_key(Path::new(".git/HEAD")));
        assert!(!snapshot.entries.contains_key(Path::new(".libra/db")));
        assert!(!snapshot.entries.contains_key(Path::new(".codex/session")));
        assert!(!snapshot.entries.contains_key(Path::new(".agents/cache")));
        assert_eq!(
            snapshot.entries.get(Path::new("nested/link.txt")),
            Some(&WorkspaceEntry::Symlink(PathBuf::from("real.txt")))
        );
    }
}
