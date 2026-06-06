//! Capture a sub-agent run's filesystem changes as a touched-file summary for
//! merge-candidate review (CEX-S2-16 / Step 2.5).
//!
//! A sub-agent runs in an isolated workspace materialized from the base
//! worktree (CEX-S2-11). When the run completes, the difference between the
//! workspace and the base IS the run's proposed change. This module computes
//! that difference as a sorted [`TouchedFile`] summary — the input from which
//! the dispatcher builds a persistent `PatchSet` and its `AgentPatchSet`
//! reference, which a `MergeCandidate` later aggregates.
//!
//! Pure file I/O: it reads the two trees and compares them, with no object
//! store, network, or clock access.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use git_internal::internal::object::patchset::{ChangeType, TouchedFile};

/// VCS-internal directories never attributed to a sub-agent's change set — the
/// `.libra` store (session JSONL, objects) and any `.git` dir are infrastructure
/// the run does not "author".
const VCS_INTERNAL_DIRS: [&str; 2] = [".libra", ".git"];

/// Diff `workspace_root` (the sub-agent's materialized, possibly-modified
/// workspace) against `base_root` (the worktree it was materialized from),
/// returning a touched-file summary sorted by path.
///
/// Each repo-relative path present only in the workspace is `Add`, present only
/// in the base is `Delete`, and present in both with differing bytes is
/// `Modify`. Line counts come from a line-level diff for text files; a binary
/// (non-UTF-8) change is still flagged with zero line counts rather than guessed.
/// VCS-internal dirs ([`VCS_INTERNAL_DIRS`]) are skipped in both trees.
pub fn workspace_touched_files(
    workspace_root: &Path,
    base_root: &Path,
) -> io::Result<Vec<TouchedFile>> {
    let workspace = collect_files(workspace_root)?;
    let base = collect_files(base_root)?;
    let mut touched = Vec::new();

    for (rel, ws_path) in &workspace {
        match base.get(rel) {
            None => touched.push(touched_file(rel, ChangeType::Add, line_count(ws_path)?, 0)),
            Some(base_path) => {
                let ws_bytes = fs::read(ws_path)?;
                let base_bytes = fs::read(base_path)?;
                if ws_bytes != base_bytes {
                    let (added, deleted) = diff_line_counts(&base_bytes, &ws_bytes);
                    touched.push(touched_file(rel, ChangeType::Modify, added, deleted));
                }
            }
        }
    }
    for (rel, base_path) in &base {
        if !workspace.contains_key(rel) {
            touched.push(touched_file(
                rel,
                ChangeType::Delete,
                0,
                line_count(base_path)?,
            ));
        }
    }

    touched.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(touched)
}

/// Build a [`TouchedFile`] directly from its (always-valid) parts. `rel` comes
/// from a directory walk so it is never empty; the public-field literal avoids
/// the fallible [`TouchedFile::new`] empty-path check.
fn touched_file(
    rel: &str,
    change_type: ChangeType,
    lines_added: u32,
    lines_deleted: u32,
) -> TouchedFile {
    TouchedFile {
        path: rel.to_string(),
        change_type,
        lines_added,
        lines_deleted,
    }
}

/// Recursively collect every regular file under `root` (skipping
/// [`VCS_INTERNAL_DIRS`]) keyed by its `/`-joined path relative to `root`. A
/// missing root yields an empty map (a run that materialized no workspace).
fn collect_files(root: &Path) -> io::Result<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    if !root.exists() {
        return Ok(files);
    }
    collect_into(root, root, &mut files)?;
    Ok(files)
}

fn collect_into(root: &Path, dir: &Path, files: &mut BTreeMap<String, PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if VCS_INTERNAL_DIRS.contains(&name.as_ref()) {
                continue;
            }
            collect_into(root, &path, files)?;
        } else if file_type.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            files.insert(rel.to_string_lossy().replace('\\', "/"), path.clone());
        }
        // Symlinks and other entries are intentionally ignored: a sub-agent's
        // change set is regular-file content, and following links could escape
        // the workspace.
    }
    Ok(())
}

/// Count lines in a whole added/deleted file: the number of `\n` bytes plus one
/// for a final line lacking a trailing newline. An empty file is zero lines.
fn line_count(path: &Path) -> io::Result<u32> {
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(0);
    }
    let newlines = bytes.iter().filter(|&&byte| byte == b'\n').count();
    // INVARIANT: `bytes` is non-empty here (the `is_empty()` early-return above).
    let trailing = u32::from(*bytes.last().expect("INVARIANT: bytes is non-empty") != b'\n');
    Ok(u32::try_from(newlines)
        .unwrap_or(u32::MAX)
        .saturating_add(trailing))
}

/// Added / deleted line counts for a modified file via a line-level diff. Binary
/// (non-UTF-8) content cannot be line-diffed, so it reports `(0, 0)` — the file
/// is still flagged `Modify`, its line deltas just unknown rather than fabricated.
fn diff_line_counts(base: &[u8], workspace: &[u8]) -> (u32, u32) {
    let (Ok(base_text), Ok(ws_text)) = (std::str::from_utf8(base), std::str::from_utf8(workspace))
    else {
        return (0, 0);
    };
    let patch = diffy::create_patch(base_text, ws_text);
    let mut added = 0u32;
    let mut deleted = 0u32;
    for hunk in patch.hunks() {
        for line in hunk.lines() {
            match line {
                diffy::Line::Insert(_) => added = added.saturating_add(1),
                diffy::Line::Delete(_) => deleted = deleted.saturating_add(1),
                diffy::Line::Context(_) => {}
            }
        }
    }
    (added, deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mk parent");
        }
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn classifies_add_modify_delete_and_skips_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("base");
        let workspace = temp.path().join("ws");

        // Base tree.
        write(&base, "keep.txt", "same\n");
        write(&base, "edit.txt", "one\ntwo\nthree\n");
        write(&base, "gone.txt", "removed line a\nremoved line b\n");
        // Workspace tree (materialized from base, then edited by the sub-agent).
        write(&workspace, "keep.txt", "same\n");
        write(
            &workspace,
            "edit.txt",
            "one\nTWO changed\nthree\nfour added\n",
        );
        write(&workspace, "new.txt", "brand new\nsecond\n");

        let touched = workspace_touched_files(&workspace, &base).expect("diff");
        // Sorted by path: edit.txt, gone.txt, new.txt — keep.txt is unchanged.
        let paths: Vec<&str> = touched.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(paths, vec!["edit.txt", "gone.txt", "new.txt"]);

        let by = |p: &str| touched.iter().find(|t| t.path == p).unwrap();
        assert_eq!(by("new.txt").change_type, ChangeType::Add);
        assert_eq!(by("new.txt").lines_added, 2);
        assert_eq!(by("gone.txt").change_type, ChangeType::Delete);
        assert_eq!(by("gone.txt").lines_deleted, 2);
        assert_eq!(by("edit.txt").change_type, ChangeType::Modify);
        // The modify diff sees one changed line (delete+insert) plus one inserted.
        assert!(by("edit.txt").lines_added >= 2);
        assert!(by("edit.txt").lines_deleted >= 1);
    }

    #[test]
    fn skips_vcs_internal_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("base");
        let workspace = temp.path().join("ws");
        write(&base, "src/main.rs", "fn main() {}\n");
        write(&workspace, "src/main.rs", "fn main() {}\n");
        // A `.libra` store change in the workspace must NOT be attributed.
        write(&workspace, ".libra/sessions/x/events.jsonl", "{}\n");

        let touched = workspace_touched_files(&workspace, &base).expect("diff");
        assert!(
            touched.is_empty(),
            "only the .libra change exists, and it must be skipped: {touched:?}",
        );
    }

    #[test]
    fn missing_workspace_root_yields_empty() {
        let temp = tempfile::tempdir().expect("tempdir");
        let base = temp.path().join("base");
        write(&base, "a.txt", "x\n");
        let touched = workspace_touched_files(&temp.path().join("nonexistent"), &base)
            .expect("missing workspace diffs as all-deleted");
        // Everything in base is reported deleted when the workspace is absent.
        assert_eq!(touched.len(), 1);
        assert_eq!(touched[0].change_type, ChangeType::Delete);
    }
}
