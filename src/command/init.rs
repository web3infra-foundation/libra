// libra/src/command/init.rs
use clap::Parser;
use git_internal::errors::GitError;
use git_internal::repository::{Repository, RepositoryConfig, RepositoryType};
use git_internal::storage::sqlite::SqliteStorage;
use std::fs;
use std::path::{Path, PathBuf};
use std::io;

/// Arguments for the `init` command
#[derive(Debug, Parser)]
pub struct InitArgs {
    /// Create a bare repository (no working directory)
    #[arg(long, default_value_t = false, help = "Create a bare repository (no working directory)")]
    bare: bool,

    /// Path to separate git directory for version control data
    #[arg(long, required = false, help = "Store version control data in the specified path")]
    separate_git_dir: Option<PathBuf>,

    /// Repository directory (default: current directory)
    #[arg(long, default_value = ".", help = "Repository directory path")]
    repo_directory: PathBuf,

    /// Initial branch name
    #[arg(long, default_value = "main", help = "Initial branch name")]
    initial_branch: String,

    /// Quiet mode (suppress non-essential output)
    #[arg(long, default_value_t = false, help = "Suppress non-essential output")]
    quiet: bool,

    /// Object format (SHA-1/SHA-256)
    #[arg(long, default_value = "sha1", help = "Object hash format (sha1/sha256)")]
    object_format: String,
}

/// CLI dispatcher entrypoint for the `init` command (matches cli.rs expected signature)
pub async fn execute(args: InitArgs) -> Result<(), GitError> {
    run(args).await
}

/// Core implementation of the init command using git-internal
async fn run(args: InitArgs) -> Result<(), GitError> {
    // Determine final storage path (comply with Libra's repo layout)
    let (storage_path, working_dir) = if args.bare {
        // Bare repo: storage path = repo directory (no working dir)
        (args.repo_directory.clone(), None)
    } else {
        // Non-bare repo: storage path = <workdir>/.libra (standard layout)
        let work_dir = args.separate_git_dir.unwrap_or(args.repo_directory.clone());
        let storage_dir = work_dir.join(".libra");
        (storage_dir, Some(work_dir))
    };

    // 1. Ensure parent directory of target path exists (create non-existent paths automatically)
    if let Some(parent) = storage_path.parent() {
        fs::create_dir_all(parent).map_err(|e| GitError::IoError(e))?;
    }

    // 2. Set directory hidden (Windows only) - hide .libra not working dir
    if !args.bare {
        // Only hide the .libra metadata directory, not the entire working dir
        set_dir_hidden(&storage_path.to_string_lossy()).map_err(|e| GitError::IoError(e))?;
    }

    // 3. Initialize repository using git-internal (Libra's native implementation)
    let repo_type = if args.bare {
        RepositoryType::Bare
    } else {
        RepositoryType::Normal
    };

    let config = RepositoryConfig {
        path: storage_path.clone(),
        bare: args.bare,
        initial_branch: args.initial_branch.clone(),
        object_format: args.object_format.clone(),
        quiet: args.quiet,
    };

    // Use git-internal's Repository instead of git2
    let mut repo = Repository::init(
        &config,
        SqliteStorage::new(&storage_path.join("libra.db")).await?
    ).await?;

    // 4. Set HEAD reference (git-internal API)
    repo.set_head(&args.initial_branch).await?;

    // 5. Success output (only if not quiet)
    if !args.quiet {
        let repo_type_str = if args.bare { "bare " } else { "" };
        let display_path = if args.bare {
            storage_path.clone()
        } else {
            working_dir.unwrap()
        };
        println!(
            "[SUCCESS] Initialized {}libra repository at: {:?} (metadata in: {:?})",
            repo_type_str,
            display_path,
            storage_path
        );
    }

    Ok(())
}

// Preserve original set_dir_hidden implementation (Windows/Unix compatibility)
#[cfg(target_os = "windows")]
fn set_dir_hidden(dir: &str) -> io::Result<()> {
    // Skip attrib command in WSL2 to avoid Buck2 build errors
    if std::env::var("WSL_DISTRO_NAME").is_ok() {
        return Ok(());
    }
    use std::process::Command;
    Command::new("attrib")
        .arg("+H")
        .arg(dir)
        .spawn()?
        .wait()?; // Wait for command execution to complete
    Ok(())
}

/// On Unix-like systems, directories starting with a dot are hidden by default
/// Therefore, this function does nothing.
#[cfg(not(target_os = "windows"))]
fn set_dir_hidden(_dir: &str) -> io::Result<()> {
    // on unix-like systems, dotfiles are hidden by default
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    /// Test normal initialization with separate git directory (git-internal implementation)
    #[tokio::test]
    async fn test_separate_git_dir_normal() {
        let temp_dir = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Failed to create temp directory: {}", e);
                return;
            }
        };
        let work_dir = temp_dir.path().join("project");
        let expected_storage = work_dir.join(".libra");
        
        let args = InitArgs {
            bare: false,
            separate_git_dir: Some(work_dir.clone()),
            repo_directory: PathBuf::from("."),
            initial_branch: "main".to_string(),
            quiet: true,
            object_format: "sha1".to_string(),
        };

        let result = execute(args).await;
        assert!(result.is_ok(), "Initialization should succeed");
        // Verify metadata is in .libra (not root)
        assert!(expected_storage.exists(), ".libra directory should be created");
        assert!(expected_storage.join("libra.db").exists(), "libra.db should be in .libra");
    }

    /// Test automatic creation of non-existent directory path
    #[tokio::test]
    async fn test_separate_git_dir_auto_create() {
        let temp_root = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Failed to create temp root directory: {}", e);
                return;
            }
        };
        let work_dir = temp_root.path().join("a/b/c/project");
        let expected_storage = work_dir.join(".libra");
        
        let args = InitArgs {
            bare: false,
            separate_git_dir: Some(work_dir.clone()),
            repo_directory: PathBuf::from("."),
            initial_branch: "main".to_string(),
            quiet: true,
            object_format: "sha1".to_string(),
        };

        let result = execute(args).await;
        assert!(result.is_ok(), "Should create non-existent directory path");
        assert!(work_dir.exists(), "Working directory should be created");
        assert!(expected_storage.exists(), ".libra metadata directory should exist");
        assert!(expected_storage.join("libra.db").exists(), "libra.db should be in .libra");
    }

    /// Test compatibility of --bare with --separate-git-dir (git-internal)
    #[tokio::test]
    async fn test_bare_with_separate_git_dir() {
        let temp_dir = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => {
                eprintln!("Failed to create temp directory: {}", e);
                return;
            }
        };
        let git_dir = temp_dir.path().join("bare-repo");
        
        let args = InitArgs {
            bare: true,
            separate_git_dir: Some(git_dir.clone()),
            repo_directory: git_dir.clone(),
            initial_branch: "main".to_string(),
            quiet: true,
            object_format: "sha1".to_string(),
        };

        let result = execute(args).await;
        assert!(result.is_ok(), "Bare repository initialization should succeed");
        // Bare repo: libra.db in root (not .libra)
        assert!(git_dir.join("libra.db").exists(), "libra.db should be in root for bare repo");
    }
}
