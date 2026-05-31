//! Filesystem sizing measurement for sub-agent workspace strategy
//! selection (CEX-S2-11).
//!
//! 子代理工作区策略选择的文件系统大小测量 (CEX-S2-11)。
//!
//! This is the **I/O sibling** of the pure [`super::workspace_strategy`]
//! module: it walks a source repository to measure the two dimensions
//! [`select_preferred_strategy`] thresholds on — object-store byte size
//! (`.git` / `.libra` ≥ 1 GiB → `Sparse`) and worktree file count
//! (≥ 100K → `Sparse`). Keeping the measurement here lets the strategy
//! policy stay free of filesystem access and trivially unit-testable.
//!
//! Measurement is **best-effort**: symlinks are not followed (we size
//! the on-disk tree, never link targets, which also avoids cycles), and
//! an entry that cannot be read is skipped rather than failing the whole
//! measurement — a transiently unreadable file must not block workspace
//! creation. A missing directory measures as zero.
//!
//! [`select_preferred_strategy`]: super::workspace_strategy::select_preferred_strategy

use std::path::Path;

use walkdir::WalkDir;

use super::workspace_strategy::WorkspaceSizing;

/// Measure a source repository's [`WorkspaceSizing`] for strategy
/// selection.
///
/// - `object_store_dir` is the repo's object store (`.libra` for Libra,
///   `.git` for a Git checkout); its recursive regular-file byte total
///   feeds `repo_size_bytes`.
/// - `worktree_files_root` is the working-tree root whose regular files
///   are counted into `worktree_file_count`. The caller chooses what to
///   point this at (e.g. the repo root); the object store is sized for
///   bytes only and is **not** included in the file count unless the
///   caller's `worktree_files_root` contains it.
///
/// Both dimensions are measured best-effort (see the module docs): the
/// result can only under-report on unreadable entries, never over-report,
/// so a measurement error biases toward the `Worktree` default rather
/// than toward an unwarranted `Sparse`.
pub fn measure_workspace_sizing(
    object_store_dir: &Path,
    worktree_files_root: &Path,
) -> WorkspaceSizing {
    WorkspaceSizing {
        repo_size_bytes: total_file_bytes(object_store_dir),
        worktree_file_count: count_regular_files(worktree_files_root),
    }
}

/// Recursive sum of regular-file sizes under `dir`, in bytes. Symlinks
/// are not followed; unreadable entries are skipped; a missing `dir`
/// sums to 0.
///
/// Uses saturating addition so a pathological aggregate near `u64::MAX`
/// can neither panic (debug overflow) nor wrap to a tiny value (release)
/// — and saturating *up* keeps the conservative direction: an
/// over-large measurement only pushes toward `Sparse`, never wrongly
/// toward `Worktree`.
fn total_file_bytes(dir: &Path) -> u64 {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .fold(0u64, u64::saturating_add)
}

/// Recursive count of regular files under `dir`. Symlinks are not
/// followed (a symlink is not counted as a file); unreadable entries are
/// skipped; a missing `dir` counts as 0.
fn count_regular_files(dir: &Path) -> u64 {
    WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count() as u64
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::internal::ai::agent_run::{
        event::WorkspaceStrategy, workspace_strategy::select_preferred_strategy,
    };

    /// A missing directory measures as zero on both dimensions (no
    /// panic, no error) — the degenerate "nothing there yet" case.
    #[test]
    fn missing_dir_measures_zero() {
        let temp = tempfile::tempdir().expect("tempdir");
        let absent = temp.path().join("does-not-exist");
        assert_eq!(total_file_bytes(&absent), 0);
        assert_eq!(count_regular_files(&absent), 0);
    }

    /// Byte total and file count are both recursive across nested
    /// directories and count only regular files (directories themselves
    /// are not files).
    #[test]
    fn measures_bytes_and_count_recursively() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::create_dir_all(root.join("a/b")).expect("mkdir");
        fs::write(root.join("top.txt"), b"hello").expect("write"); // 5 bytes
        fs::write(root.join("a/mid.txt"), b"world!").expect("write"); // 6 bytes
        fs::write(root.join("a/b/deep.bin"), [0u8; 100]).expect("write"); // 100 bytes

        assert_eq!(total_file_bytes(root), 5 + 6 + 100);
        assert_eq!(count_regular_files(root), 3);
    }

    /// `measure_workspace_sizing` sizes the object-store dir for bytes
    /// and the worktree root for file count independently, and the
    /// result drives `select_preferred_strategy` correctly for a small
    /// repo (→ Worktree).
    #[test]
    fn measure_combines_object_store_bytes_and_worktree_count() {
        let temp = tempfile::tempdir().expect("tempdir");
        let object_store = temp.path().join(".libra");
        let worktree = temp.path().join("work");
        fs::create_dir_all(&object_store).expect("mkdir object store");
        fs::create_dir_all(&worktree).expect("mkdir worktree");
        fs::write(object_store.join("pack.idx"), [7u8; 2048]).expect("write pack");
        fs::write(worktree.join("main.rs"), b"fn main() {}").expect("write src");
        fs::write(worktree.join("lib.rs"), b"// lib").expect("write lib");

        let sizing = measure_workspace_sizing(&object_store, &worktree);
        assert_eq!(sizing.repo_size_bytes, 2048);
        assert_eq!(sizing.worktree_file_count, 2);
        // A 2 KiB store + 2 files is well under both thresholds → Worktree.
        assert_eq!(
            select_preferred_strategy(sizing),
            WorkspaceStrategy::Worktree
        );
    }

    /// Symlinks are not followed: a symlinked file is not counted as a
    /// regular file and its target bytes are not summed (prevents
    /// double-counting and link-cycle hangs). Unix-only — `std::os::unix`
    /// symlink API.
    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::write(root.join("real.txt"), [1u8; 50]).expect("write real"); // 50 bytes, 1 file
        symlink(root.join("real.txt"), root.join("link.txt")).expect("symlink");

        // Only the real file is counted; the symlink is neither a regular
        // file (count) nor re-summed (bytes).
        assert_eq!(count_regular_files(root), 1);
        assert_eq!(total_file_bytes(root), 50);
    }
}
