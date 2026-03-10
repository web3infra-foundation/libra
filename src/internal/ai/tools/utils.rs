//! Utility functions for tool handlers.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::{
    internal::ai::tools::error::{ToolError, ToolResult},
    utils,
};

/// Validate that a path is within the allowed working directory.
///
/// This ensures that tool operations cannot access files outside
/// the designated working directory for security.
pub fn validate_path(path: &Path, working_dir: &Path) -> ToolResult<()> {
    if !path.is_absolute() {
        return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
    }

    let working_dir_canonical = canonicalize_for_boundary(working_dir)?;
    let path_canonical = canonicalize_for_boundary(path)?;

    // Check if path is within canonicalized working directory boundaries.
    if !utils::util::is_sub_path(&path_canonical, &working_dir_canonical) {
        return Err(ToolError::PathOutsideWorkingDir(path.to_path_buf()));
    }

    Ok(())
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
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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
}
