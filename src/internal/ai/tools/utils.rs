//! Utility functions for tool handlers.

use std::path::Path;

use crate::internal::ai::tools::error::{ToolError, ToolResult};
use crate::utils;

/// Validate that a path is within the allowed working directory.
///
/// This ensures that tool operations cannot access files outside
/// the designated working directory for security.
pub fn validate_path(path: &Path, working_dir: &Path) -> ToolResult<()> {
    if !path.is_absolute() {
        return Err(ToolError::PathNotAbsolute(path.to_path_buf()));
    }

    // Check if path is within working directory
    if !utils::util::is_sub_path(path, working_dir) {
        return Err(ToolError::PathOutsideWorkingDir(path.to_path_buf()));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
