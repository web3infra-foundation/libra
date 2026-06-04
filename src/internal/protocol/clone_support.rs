//! Support helpers for `libra clone` local-object reuse (`--reference`,
//! `--reference-if-able`, `--shared`, `--local`).
//!
//! Libra's object reader (`ClientStorage::get`) has no `info/alternates`
//! fallback, so these flags use **copy semantics**: the referenced repository's
//! objects are copied into the new clone's tiered storage and the clone carries
//! no long-term alternates dependency. This module locates a source object
//! directory and copies its loose objects and pack files into the destination,
//! rejecting symbolic-link sources to prevent cross-account path escapes.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Maximum byte length accepted for a reference/local source path. Bounds input
/// handling and mirrors the 4 KiB cap used elsewhere in the clone surface.
pub const REFERENCE_PATH_MAX_LEN: usize = 4096;

/// Errors raised while validating or copying a local object source. Kept free of
/// command-layer types so the protocol layer stays decoupled; `command::clone`
/// maps these onto its `CloneError` variants (and thus exit codes).
#[derive(Debug)]
pub enum CloneSupportError {
    /// The source path (or an entry under it) is a symbolic link — rejected to
    /// prevent local cross-account privilege escapes.
    Symlink(PathBuf),
    /// The (canonical) source path exceeds [`REFERENCE_PATH_MAX_LEN`].
    PathTooLong(usize),
    /// The source path does not resolve to a repository object directory.
    NotARepository(PathBuf),
    /// An I/O error occurred while inspecting or copying objects.
    Io(io::Error),
}

impl std::fmt::Display for CloneSupportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloneSupportError::Symlink(path) => write!(
                f,
                "symbolic links in the reference object source '{}' are blocked for security reasons",
                path.display()
            ),
            CloneSupportError::PathTooLong(len) => write!(
                f,
                "reference object source path exceeds the {REFERENCE_PATH_MAX_LEN}-byte limit ({len} bytes)"
            ),
            CloneSupportError::NotARepository(path) => write!(
                f,
                "reference source '{}' is not a libra or git repository",
                path.display()
            ),
            CloneSupportError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl From<io::Error> for CloneSupportError {
    fn from(error: io::Error) -> Self {
        CloneSupportError::Io(error)
    }
}

/// Canonicalize `path` and reject it when the original entry is a symbolic link
/// or the canonical path is over-long. Returns the canonical absolute path so
/// callers never operate on a relative or symlinked source.
pub fn check_local_security(path: &Path) -> Result<PathBuf, CloneSupportError> {
    // `symlink_metadata` does not follow the final component, so a symlinked
    // source directory is detected before canonicalization resolves it.
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(CloneSupportError::Symlink(path.to_path_buf()));
    }
    let canonical = fs::canonicalize(path)?;
    let len = canonical.as_os_str().len();
    if len > REFERENCE_PATH_MAX_LEN {
        return Err(CloneSupportError::PathTooLong(len));
    }
    Ok(canonical)
}

/// Resolve the object directory of a reference repository, supporting libra
/// (`.libra/objects`), bare (`objects`), and git working-tree (`.git/objects`)
/// layouts. `reference` should already be canonical (see [`check_local_security`]).
/// A symlinked object directory is rejected so the security guard cannot be
/// bypassed by pointing the object root itself at another location.
pub fn resolve_reference_objects_dir(reference: &Path) -> Result<PathBuf, CloneSupportError> {
    for candidate in [
        reference.join(".libra").join("objects"),
        reference.join("objects"),
        reference.join(".git").join("objects"),
    ] {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CloneSupportError::Symlink(candidate));
            }
            Ok(metadata) if metadata.is_dir() => return Ok(candidate),
            _ => continue,
        }
    }
    Err(CloneSupportError::NotARepository(reference.to_path_buf()))
}

/// Reject a symlinked object-source root before any copy/hardlink walk.
fn reject_symlinked_root(src_objects: &Path) -> Result<(), CloneSupportError> {
    if let Ok(metadata) = fs::symlink_metadata(src_objects)
        && metadata.file_type().is_symlink()
    {
        return Err(CloneSupportError::Symlink(src_objects.to_path_buf()));
    }
    Ok(())
}

/// Copy every loose object and pack file from `src_objects` into `dest_objects`,
/// preserving the `ab/cdef…` loose layout and the `pack/` subtree. Symlinked
/// entries are rejected, paths are length-capped, and existing destination files
/// are left intact (objects are content-addressed, so a same-named file is the
/// same object). Returns the number of files copied.
pub fn copy_objects(src_objects: &Path, dest_objects: &Path) -> Result<usize, CloneSupportError> {
    reject_symlinked_root(src_objects)?;
    fs::create_dir_all(dest_objects)?;
    let mut copied = 0usize;
    copy_dir_recursive(src_objects, dest_objects, &mut copied)?;
    Ok(copied)
}

fn copy_dir_recursive(
    src: &Path,
    dest: &Path,
    copied: &mut usize,
) -> Result<(), CloneSupportError> {
    let mut report = LinkReport::default();
    reuse_dir_recursive(src, dest, false, &mut report)?;
    *copied += report.copied;
    Ok(())
}

/// Outcome of [`link_objects`]: how many files were hardlinked versus copied
/// (e.g. when a cross-filesystem hardlink fell back to a copy).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct LinkReport {
    pub hardlinked: usize,
    pub copied: usize,
}

/// Hardlink every loose object and pack file from `src_objects` into
/// `dest_objects`, falling back to a copy when a hardlink is not possible (for
/// example across filesystems). Same security guards as [`copy_objects`].
pub fn link_objects(
    src_objects: &Path,
    dest_objects: &Path,
) -> Result<LinkReport, CloneSupportError> {
    reject_symlinked_root(src_objects)?;
    fs::create_dir_all(dest_objects)?;
    let mut report = LinkReport::default();
    reuse_dir_recursive(src_objects, dest_objects, true, &mut report)?;
    Ok(report)
}

fn reuse_dir_recursive(
    src: &Path,
    dest: &Path,
    hardlink: bool,
    report: &mut LinkReport,
) -> Result<(), CloneSupportError> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(CloneSupportError::Symlink(entry.path()));
        }
        let src_path = entry.path();
        if src_path.as_os_str().len() > REFERENCE_PATH_MAX_LEN {
            return Err(CloneSupportError::PathTooLong(src_path.as_os_str().len()));
        }
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir_all(&dest_path)?;
            reuse_dir_recursive(&src_path, &dest_path, hardlink, report)?;
        } else if file_type.is_file() {
            // Content-addressed: an existing destination file is the same object.
            if dest_path.exists() {
                continue;
            }
            if hardlink {
                match fs::hard_link(&src_path, &dest_path) {
                    Ok(()) => report.hardlinked += 1,
                    Err(_) => {
                        // Cross-filesystem (EXDEV) or unsupported — fall back to copy.
                        fs::copy(&src_path, &dest_path)?;
                        report.copied += 1;
                    }
                }
            } else {
                fs::copy(&src_path, &dest_path)?;
                report.copied += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_objects_dir_prefers_libra_layout() {
        let dir = tempfile::tempdir().unwrap();
        let objects = dir.path().join(".libra").join("objects");
        fs::create_dir_all(&objects).unwrap();
        let resolved = resolve_reference_objects_dir(dir.path()).unwrap();
        assert_eq!(resolved, objects);
    }

    #[test]
    fn resolve_objects_dir_supports_bare_and_git_layouts() {
        let bare = tempfile::tempdir().unwrap();
        fs::create_dir_all(bare.path().join("objects")).unwrap();
        assert_eq!(
            resolve_reference_objects_dir(bare.path()).unwrap(),
            bare.path().join("objects")
        );

        let git = tempfile::tempdir().unwrap();
        fs::create_dir_all(git.path().join(".git").join("objects")).unwrap();
        assert_eq!(
            resolve_reference_objects_dir(git.path()).unwrap(),
            git.path().join(".git").join("objects")
        );
    }

    #[test]
    fn resolve_objects_dir_rejects_non_repository() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_reference_objects_dir(dir.path()),
            Err(CloneSupportError::NotARepository(_))
        ));
    }

    #[test]
    fn copy_objects_copies_loose_and_pack_files() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let src_objects = src.path().join("objects");
        fs::create_dir_all(src_objects.join("ab")).unwrap();
        fs::create_dir_all(src_objects.join("pack")).unwrap();
        fs::write(src_objects.join("ab").join("cdef"), b"loose").unwrap();
        fs::write(src_objects.join("pack").join("pack-1.pack"), b"PACK").unwrap();
        fs::write(src_objects.join("pack").join("pack-1.idx"), b"IDX").unwrap();

        let dest_objects = dest.path().join("objects");
        let copied = copy_objects(&src_objects, &dest_objects).unwrap();
        assert_eq!(copied, 3);
        assert_eq!(
            fs::read(dest_objects.join("ab").join("cdef")).unwrap(),
            b"loose"
        );
        assert!(dest_objects.join("pack").join("pack-1.pack").exists());

        // Re-copy is idempotent: existing content-addressed files are skipped.
        let copied_again = copy_objects(&src_objects, &dest_objects).unwrap();
        assert_eq!(copied_again, 0);
    }

    #[cfg(unix)]
    #[test]
    fn link_objects_hardlinks_same_filesystem() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let src_objects = dir.path().join("src-objects");
        fs::create_dir_all(src_objects.join("pack")).unwrap();
        fs::write(src_objects.join("pack").join("pack-1.pack"), b"PACK").unwrap();

        let dest_objects = dir.path().join("dest-objects");
        let report = link_objects(&src_objects, &dest_objects).unwrap();
        assert_eq!(report.hardlinked, 1);
        assert_eq!(report.copied, 0);

        // Hardlinked files share an inode with the source.
        let src_ino = fs::metadata(src_objects.join("pack").join("pack-1.pack"))
            .unwrap()
            .ino();
        let dest_ino = fs::metadata(dest_objects.join("pack").join("pack-1.pack"))
            .unwrap()
            .ino();
        assert_eq!(src_ino, dest_ino, "hardlinked object must share an inode");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_objects_dir_rejects_symlinked_objects_root() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real_objects = dir.path().join("real-objects");
        fs::create_dir_all(&real_objects).unwrap();
        // `<repo>/.libra/objects` is a symlink pointing elsewhere.
        fs::create_dir_all(dir.path().join("repo").join(".libra")).unwrap();
        symlink(
            &real_objects,
            dir.path().join("repo").join(".libra").join("objects"),
        )
        .unwrap();
        assert!(matches!(
            resolve_reference_objects_dir(&dir.path().join("repo")),
            Err(CloneSupportError::Symlink(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn copy_objects_rejects_symlinked_root() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link-objects");
        symlink(&real, &link).unwrap();
        assert!(matches!(
            copy_objects(&link, &dir.path().join("dest")),
            Err(CloneSupportError::Symlink(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn check_local_security_rejects_symlink_source() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        symlink(&real, &link).unwrap();
        assert!(matches!(
            check_local_security(&link),
            Err(CloneSupportError::Symlink(_))
        ));
    }
}
