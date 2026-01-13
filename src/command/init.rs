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
    // Determine final storage path (honor separate-git-dir if provided)
    let storage_path = match args.separate_git_dir {
        Some(git_dir) => git_dir,
        None => args.repo_directory.clone(),
    };

    // 1. Create directory structure (including parent directories)
    if let Some(parent) = storage_path.parent() {
        fs::create_dir_all(parent).map_err(|e| GitError::IoError(e))?;
    }

    // 2. Set directory hidden (Windows only) - preserve original implementation
    if !args.bare {
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
        println!(
            "[SUCCESS] Initialized {}libra repository at: {:?}",
            repo_type_str,
            storage_path
        );
    }

    Ok(())
}

// Preserve original set_dir_hidden implementation (Windows/Unix compatibility)
#[cfg(target_os = "windows")]
fn set_dir_hidden(dir: &str) -> io::Result<()> {
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

    /// Test normal initialization with separate git directory (git-internal implementation)
    #[tokio::test]
    async fn test_separate_git_dir_normal() {
        let temp_dir = TempDir::new().unwrap();
        let git_dir = temp_dir.path().join("git-data");
        
        let args = InitArgs {
            bare: false,
            separate_git_dir: Some(git_dir.clone()),
            repo_directory: PathBuf::from("."),
            initial_branch: "main".to_string(),
            quiet: true,
            object_format: "sha1".to_string(),
        };

        let result = execute(args).await;
        assert!(result.is_ok());
        // Verify Libra's native sqlite database (git-internal) instead of git2's config
        assert!(git_dir.join("libra.db").exists());
    }

    /// Test automatic creation of non-existent directory path
    #[tokio::test]
    async fn test_separate_git_dir_auto_create() {
        let temp_root = TempDir::new().unwrap();
        let non_exist_dir = temp_root.path().join("a/b/c/libra-repo");
        
        let args = InitArgs {
            bare: false,
            separate_git_dir: Some(non_exist_dir.clone()),
            repo_directory: PathBuf::from("."),
            initial_branch: "main".to_string(),
            quiet: true,
            object_format: "sha1".to_string(),
        };

        let result = execute(args).await;
        assert!(result.is_ok());
        assert!(non_exist_dir.exists());
        assert!(non_exist_dir.join("libra.db").exists());
    }

    /// Test compatibility of --bare with --separate-git-dir (git-internal)
    #[tokio::test]
    async fn test_bare_with_separate_git_dir() {
        let temp_dir = TempDir::new().unwrap();
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
        assert!(result.is_ok());
        assert!(git_dir.join("libra.db").exists());
    }
}
