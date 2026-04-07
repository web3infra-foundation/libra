//! Utility functions for tool handlers.

use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
};

use crate::{
    internal::ai::tools::error::{ToolError, ToolResult},
    utils::{self, util},
};

/// Validate that a path is within the allowed working directory.
///
/// This ensures that tool operations cannot access files outside
/// the designated working directory for security.
pub fn validate_path(path: &Path, working_dir: &Path) -> ToolResult<()> {
    if !path.is_absolute() {
        return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
    }

    if is_reserved_metadata_path(path, working_dir) {
        return Err(ToolError::PathReserved(path.to_path_buf()));
    }

    if !is_within_working_dir(path, working_dir)? {
        return Err(ToolError::PathOutsideWorkingDir(path.to_path_buf()));
    }

    Ok(())
}

/// Returns true if `path` stays inside `working_dir` after boundary canonicalization.
pub fn is_within_working_dir(path: &Path, working_dir: &Path) -> ToolResult<bool> {
    let working_dir_canonical = canonicalize_for_boundary(working_dir)?;
    let path_canonical = canonicalize_for_boundary(path)?;
    Ok(utils::util::is_sub_path(
        &path_canonical,
        &working_dir_canonical,
    ))
}

/// Resolve an absolute or relative path inside the working directory.
///
/// Relative paths are interpreted from `working_dir`. The returned path is
/// always absolute and must remain within the working directory boundary.
pub fn resolve_path(path: &Path, working_dir: &Path) -> ToolResult<PathBuf> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };
    validate_path(&resolved, working_dir)?;
    Ok(resolved)
}

fn is_reserved_metadata_path(path: &Path, working_dir: &Path) -> bool {
    let normalized_working_dir = normalize_lexical_absolute(working_dir);
    let normalized_path = normalize_lexical_absolute(path);
    let relative = match normalized_path.strip_prefix(&normalized_working_dir) {
        Ok(relative) => relative,
        Err(_) => return false,
    };

    matches!(
        relative.components().next(),
        Some(Component::Normal(name)) if name == OsStr::new(util::ROOT_DIR)
    )
}

fn canonicalize_for_boundary(path: &Path) -> ToolResult<PathBuf> {
    if path.exists() {
        return path.canonicalize().map_err(ToolError::Io);
    }

    let mut suffix = Vec::<OsString>::new();
    let mut cursor = path;
    while !cursor.exists() {
        let name = cursor.file_name().ok_or_else(|| {
            ToolError::ExecutionFailed(format!(
                "cannot resolve path boundary for '{}'",
                path.display()
            ))
        })?;
        suffix.push(name.to_os_string());
        cursor = cursor.parent().ok_or_else(|| {
            ToolError::ExecutionFailed(format!(
                "cannot resolve parent path for '{}'",
                path.display()
            ))
        })?;
    }

    let mut canonical = cursor.canonicalize().map_err(ToolError::Io)?;
    for part in suffix.iter().rev() {
        canonical.push(part);
    }
    Ok(normalize_lexical_absolute(&canonical))
}

fn normalize_lexical_absolute(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(component.as_os_str())),
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    normalized.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_validate_path_absolute() {
        let working_dir = PathBuf::from("/tmp/work");
        let path = PathBuf::from("/tmp/work/file.txt");
        assert!(validate_path(&path, &working_dir).is_ok());
    }

    #[test]
    fn test_validate_path_relative() {
        let working_dir = PathBuf::from("/tmp/work");
        let path = PathBuf::from("relative/file.txt");
        assert!(matches!(
            validate_path(&path, &working_dir),
            Err(ToolError::PathNotAbsolute(_))
        ));
    }

    #[test]
    fn test_validate_path_outside_working_dir() {
        let working_dir = PathBuf::from("/tmp/work");
        let path = PathBuf::from("/etc/passwd");
        // The result depends on whether the path is a subpath of working_dir
        // Since /etc is not under /tmp/work, this should fail
        let result = validate_path(&path, &working_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_path_relative_to_working_dir() {
        let working_dir = PathBuf::from("/tmp/work");
        let path = PathBuf::from("src/main.rs");
        let resolved = resolve_path(&path, &working_dir).unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/work/src/main.rs"));
    }

    #[test]
    fn test_validate_path_rejects_repository_metadata() {
        let temp = tempdir().unwrap();
        let working_dir = temp.path().to_path_buf();
        fs::create_dir_all(working_dir.join(util::ROOT_DIR)).unwrap();
        let reserved_path = working_dir.join(util::ROOT_DIR).join("refs").join("heads");

        let result = validate_path(&reserved_path, &working_dir);

        assert!(matches!(result, Err(ToolError::PathReserved(path)) if path == reserved_path));
    }
}
