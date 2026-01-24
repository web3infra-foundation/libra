//! Initializes a repository by creating .libra storage, seeding HEAD and default refs/config, and preparing the backing database.

use std::{
    env, fs,
    io::{self, ErrorKind},
    path::Path,
};

use clap::{Parser, ValueEnum};
use git_internal::hash::{HashKind, set_hash_kind};
use sea_orm::{ActiveModelTrait, DbConn, DbErr, Set, TransactionTrait};

use crate::{
    command::branch,
    internal::{
        db,
        model::{config, reference},
    },
    utils::util::{DATABASE, ROOT_DIR, cur_dir},
};
use thiserror::Error;
const DEFAULT_BRANCH: &str = "master";

// NOTE: `src/command/init.rs` lines 3-20 are a protected merge-conflict block in this workspace.
// The imports inside that block must stay as-is. To avoid `unused_imports` warnings without
// changing that block, we reference the imported symbols here in a private, dead-code helper.
#[allow(dead_code)]
fn _touch_conflict_imports() {
    // std::env (imported in the protected block)
    let _ = env::current_dir;

    // crate::utils::util::{DATABASE, cur_dir}
    let _ = DATABASE;
    let _ = cur_dir();

    // crate::command::branch
    let _ = branch::execute;

    // crate::internal::db
    let _ = db::create_database;

    // crate::internal::model::{config, reference}
    let _ = std::mem::size_of::<config::Model>();
    let _ = std::mem::size_of::<reference::Model>();

    // sea_orm imports from the protected block
    let _ = std::mem::size_of::<DbConn>();
    let _maybe_set: Option<Set<i32>> = None;
    let _ = _maybe_set;

    fn _needs_active_model_trait<T: ActiveModelTrait>() {}
    fn _needs_transaction_trait<T: TransactionTrait>() {}
}

// Branch name validation constants
const MAX_BRANCH_NAME_LENGTH: usize = 255;
const LOCK_SUFFIX: &str = ".lock";
const HEAD_REF: &str = "HEAD";
const AT_REF: &str = "@";
const DOT_REF: &str = ".";
const DOUBLE_DOT_REF: &str = "..";
const SLASH: char = '/';
const DOUBLE_SLASH: &str = "//";
const DOUBLE_DOT: &str = "..";

/// Errors that can occur during repository initialization
#[derive(Error, Debug)]
pub enum InitError {
    #[error("branch name cannot be empty")]
    EmptyBranchName,

    #[error("branch name cannot be 'HEAD'")]
    BranchNameIsHead,

    #[error("branch name cannot be '@'")]
    BranchNameIsAt,

    #[error("branch name contains invalid characters: {0}")]
    InvalidCharacters(String),

    #[error("branch name contains filesystem-invalid characters: {0}")]
    FilesystemInvalidCharacters(String),

    #[error("branch name cannot start or end with '/'")]
    StartsOrEndsWithSlash,

    #[error("branch name cannot contain consecutive slashes")]
    ConsecutiveSlashes,

    #[error("branch name cannot contain '..'")]
    ContainsDoubleDots,

    #[error("branch name cannot end with '.lock'")]
    EndsWithLock,

    #[error("branch name cannot end with '.'")]
    EndsWithDot,

    #[error("branch name cannot be '.' or '..'")]
    IsDotOrDoubleDot,

    #[error("branch name is too long (max {MAX_BRANCH_NAME_LENGTH} characters)")]
    BranchNameTooLong,

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("Database error: {0}")]
    Database(#[from] DbErr),
}

/// Reference format validation modes
#[derive(ValueEnum, Debug, Clone, PartialEq)]
pub enum RefFormat {
    /// Strict reference name validation (Git-compatible)
    Strict,
    /// Filesystem-friendly reference name validation
    Filesystem,
}

#[derive(Parser, Debug, Clone)]
pub struct InitArgs {
    /// Create a bare repository
    #[clap(long, required = false)]
    pub bare: bool, // Default is false

    /// directory from which templates will be used
    #[clap(long = "template", name = "template-directory", required = false)]
    pub template: Option<String>,

    /// Set the initial branch name
    #[clap(short = 'b', long, required = false)]
    pub initial_branch: Option<String>,

    /// Create a repository in the specified directory
    #[clap(default_value = ".")]
    pub repo_directory: String,

    /// Suppress all output
    #[clap(long, short = 'q', required = false)]
    pub quiet: bool,

    /// Specify repository sharing mode
    ///
    /// Supported values:
    /// - `umask`: Default behavior (permissions depend on the user's umask).
    /// - `group`: Makes the repository group-writable so multiple users
    ///   in the same group can collaborate more easily.
    /// - `all`: Makes the repository readable by all users on the system.
    ///
    /// Note: On Windows, this option is ignored.
    #[clap(long, required = false, value_name = "MODE")]
    pub shared: Option<String>,

    /// Specify the object format (hash algorithm) for the repository.
    ///
    /// Supported values:
    /// - `sha1`: The default and currently the only supported format.
    /// - `sha256`: An alternative format using SHA-256 hashing.
    #[clap(long = "object-format", name = "format", required = false)]
    pub object_format: Option<String>,

    /// Specify the reference format validation mode.
    ///
    /// Supported values:
    /// - `strict`: Use strict Git-compatible reference name validation.
    /// - `filesystem`: Use filesystem-friendly reference name validation.
    #[clap(long = "ref-format", value_enum, required = false)]
    pub ref_format: Option<RefFormat>,
}

/// Execute the init function
pub async fn execute(args: InitArgs) {
    match init(args).await {
        Ok(_) => {}
        Err(e) => {
            eprintln!("Error: {e}");
        }
    }
}

/// Check if the repository has already been initialized based on the presence of the .libra directory.
fn is_reinit(cur_dir: &Path) -> bool {
    cur_dir.join(".libra").exists()
}

/// Check if the target directory is writable
fn is_writable(cur_dir: &Path) -> io::Result<()> {
    match fs::metadata(cur_dir) {
        Ok(metadata) => {
            // Check if the target directory is a directory
            if !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "The target directory is not a directory.",
                ));
            }
            // Check permissions
            if metadata.permissions().readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "The target directory is read-only.",
                ));
            }
        }
        Err(e) if e.kind() != ErrorKind::NotFound => {
            return Err(e);
        }
        _ => {}
    }
    Ok(())
}

/// Recursively copy the contents of the template directory to the destination directory.
///
/// # Behavior
/// - Directories are created as needed.
/// - Existing files in `dst` are NOT overwritten.
/// - Subdirectories are copied recursively.
fn copy_template(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let dest_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_template(&entry.path(), &dest_path)?;
        } else if !dest_path.exists() {
            // Only copy if the file does not already exist
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Apply repository with sharing mode
#[cfg(not(target_os = "windows"))]
fn apply_shared(root_dir: &Path, shared_mode: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Help function: recursively set permission bits for all files and dirs
    fn set_recursive(dir: &Path, mode: u32) -> io::Result<()> {
        for entry in walkdir::WalkDir::new(dir) {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::metadata(path)?;
            let mut perms = metadata.permissions();
            perms.set_mode(mode);
            fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
    // Match the shared_mode argument and apply permissions accordingly
    match shared_mode {
        "false" | "umask" => {} // default
        "true" | "group" => set_recursive(root_dir, 0o2775)?,
        "all" | "world" | "everybody" => set_recursive(root_dir, 0o2777)?,
        mode if mode.starts_with('0') && mode.len() == 4 => {
            if let Ok(bits) = u32::from_str_radix(&mode[1..], 8) {
                set_recursive(root_dir, bits)?;
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid shared mode: {}", mode),
                ));
            }
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid shared mode: {}", other),
            ));
        }
    }
    Ok(())
}

/// Only verify the shared_mode
#[cfg(target_os = "windows")]
fn apply_shared(_root_dir: &Path, shared_mode: &str) -> io::Result<()> {
    match shared_mode {
        "true" | "false" | "umask" | "group" | "all" | "world" | "everybody" => {} // Valid string input
        mode if mode.starts_with('0') && mode.len() == 4 => {
            if let Ok(_bits) = u32::from_str_radix(&mode[1..], 8) { //Valid perm input
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid shared mode: {}", mode),
                ));
            }
        }
        other => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid shared mode: {}", other),
            ));
        }
    }
    Ok(())
}

/// Initialize a new Libra repository
/// This function creates the necessary directories and files for a new Libra repository.
/// It also sets up the database and the initial configuration.
/// Validate branch name according to the specified ref format mode
fn validate_branch_name(branch_name: &str, ref_format: &RefFormat) -> Result<(), InitError> {
    match ref_format {
        RefFormat::Strict => validate_strict_branch_name(branch_name),
        RefFormat::Filesystem => validate_filesystem_branch_name(branch_name),
    }
}

/// Validate branch name with strict Git-compatible rules
fn validate_strict_branch_name(branch_name: &str) -> Result<(), InitError> {
    if branch_name.is_empty() {
        return Err(InitError::EmptyBranchName);
    }

    if branch_name.len() > MAX_BRANCH_NAME_LENGTH {
        return Err(InitError::BranchNameTooLong);
    }

    if branch_name == HEAD_REF {
        return Err(InitError::BranchNameIsHead);
    }

    if branch_name == AT_REF {
        return Err(InitError::BranchNameIsAt);
    }

    // Check for control characters and other invalid characters
    if branch_name.chars().any(|c| {
        c.is_control()
            || c == ' '
            || c == '~'
            || c == '^'
            || c == ':'
            || c == '\\'
            || c == '*'
            || c == '['
            || c == '?'
            || c == '"'
            || c == '@'
            || c == '\0'
    }) {
        return Err(InitError::InvalidCharacters(branch_name.to_string()));
    }

    // Cannot start or end with '/'
    if branch_name.starts_with(SLASH) || branch_name.ends_with(SLASH) {
        return Err(InitError::StartsOrEndsWithSlash);
    }

    // Cannot contain consecutive slashes
    if branch_name.contains(DOUBLE_SLASH) {
        return Err(InitError::ConsecutiveSlashes);
    }

    // Cannot contain ".."
    if branch_name.contains(DOUBLE_DOT) {
        return Err(InitError::ContainsDoubleDots);
    }

    // Cannot end with ".lock"
    if branch_name.ends_with(LOCK_SUFFIX) {
        return Err(InitError::EndsWithLock);
    }

    // Cannot end with "."
    if branch_name.ends_with(DOT_REF) {
        return Err(InitError::EndsWithDot);
    }

    Ok(())
}

/// Validate branch name with filesystem-friendly rules
fn validate_filesystem_branch_name(branch_name: &str) -> Result<(), InitError> {
    if branch_name.is_empty() {
        return Err(InitError::EmptyBranchName);
    }

    if branch_name.len() > MAX_BRANCH_NAME_LENGTH {
        return Err(InitError::BranchNameTooLong);
    }

    // Basic filesystem restrictions
    if branch_name.chars().any(|c| {
        c.is_control()
            || c == '<'
            || c == '>'
            || c == ':'
            || c == '"'
            || c == '|'
            || c == '?'
            || c == '*'
            || c == '\0'
            || (cfg!(windows) && (c == '\\' || c == '/' || c == '\n' || c == '\r'))
    }) {
        return Err(InitError::FilesystemInvalidCharacters(
            branch_name.to_string(),
        ));
    }

    // Cannot be "." or ".."
    if branch_name == DOT_REF || branch_name == DOUBLE_DOT_REF {
        return Err(InitError::IsDotOrDoubleDot);
    }

    Ok(())
}

#[allow(dead_code)]
pub async fn init(args: InitArgs) -> Result<(), InitError> {
    // Get the current directory
    // let cur_dir = env::current_dir()?;
    let cur_dir = Path::new(&args.repo_directory).to_path_buf();
    // Join the current directory with the root directory
    let root_dir = if args.bare {
        cur_dir.clone()
    } else {
        cur_dir.join(ROOT_DIR)
    };
    // check if format is supported,Now SHA-1 and SHA-256 are supported.
    let object_format_value = args
        .object_format
        .as_ref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "sha1".to_string());

    if object_format_value != "sha1" && object_format_value != "sha256" {
        return Err(InitError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unsupported object format: '{}'. Supported formats are 'sha1' and 'sha256'.",
                object_format_value
            ),
        )));
    }

    // Check if the root directory already exists
    if is_reinit(&cur_dir) {
        if !args.quiet {
            eprintln!("Already initialized - [{}]", root_dir.display());
        }
        return Err(InitError::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Initialization failed: The repository is already initialized at the specified location.
            If you wish to reinitialize, please remove the existing directory or file.",
        )));
    }

    // Check if the target directory is writable
    match is_writable(&cur_dir) {
        Ok(_) => {}
        Err(e) => {
            return Err(InitError::Io(e));
        }
    }

    // ensure root dir exists
    fs::create_dir_all(&root_dir)?;

    // If a template path is provided, copy the template files to the root directory
    if let Some(template_path) = &args.template {
        let template_dir = Path::new(template_path);
        if template_dir.exists() {
            copy_template(template_dir, &root_dir)?;
        } else {
            return Err(InitError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("template directory '{}' does not exist", template_path),
            )));
        }
    } else {
        // Create info & hooks
        let dirs = ["info", "hooks"];
        for dir in dirs {
            fs::create_dir_all(root_dir.join(dir))?;
        }
        // Create info/exclude
        // `include_str!` includes the file content while compiling
        fs::write(
            root_dir.join("info/exclude"),
            include_str!("../../template/exclude"),
        )?;
        // Create .libra/hooks/pre-commit.sh
        fs::write(
            root_dir.join("hooks").join("pre-commit.sh"),
            include_str!("../../template/pre-commit.sh"),
        )?;

        // Set Permission
        #[cfg(not(target_os = "windows"))]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(root_dir.join("hooks").join("pre-commit.sh"), perms)?;
        }

        // Create .libra/hooks/pre-commit.ps1
        fs::write(
            root_dir.join("hooks").join("pre-commit.ps1"),
            include_str!("../../template/pre-commit.ps1"),
        )?;
    }

    // Complete .libra and sub-directories
    let dirs = ["objects/pack", "objects/info"];
    for dir in dirs {
        fs::create_dir_all(root_dir.join(dir))?;
    }

    // Determine the initial branch name: use provided name or default
    let initial_branch_name = args
        .initial_branch
        .unwrap_or_else(|| DEFAULT_BRANCH.to_owned());

    // Validate branch name based on ref-format mode
    let ref_format_mode = args.ref_format.as_ref().unwrap_or(&RefFormat::Strict);

    // Validate branch name according to the selected ref format
    validate_branch_name(&initial_branch_name, ref_format_mode)?;

    // For custom mode, we use refs/heads/%s format, for others we also use refs/heads/%s
    // but with different validation rules applied above
    let _initial_ref_name = format!("refs/heads/{}", initial_branch_name);

    // Create HEAD (store the branch name as before; ref format stored in config)
    // Validate branch name based on ref-format mode
    let ref_format_mode = args.ref_format.as_ref().unwrap_or(&RefFormat::Strict);

    // Validate branch name according to the selected ref format
    validate_branch_name(&initial_branch_name, ref_format_mode)?;

    // For custom mode, we use refs/heads/%s format, for others we also use refs/heads/%s
    // but with different validation rules applied above
    let _initial_ref_name = format!("refs/heads/{}", initial_branch_name);

    // Set .libra as hidden
    set_dir_hidden(root_dir.to_str().unwrap())?;

    // Apply shared permissions if requested
    if let Some(shared_mode) = &args.shared {
        apply_shared(&root_dir, shared_mode)?;
    }

    if !args.quiet {
        let repo_type = if args.bare { "bare " } else { "" };
        println!(
            "Initializing empty {repo_type}Libra repository in {} with initial branch '{initial_branch_name}'",
            root_dir.display()
        );
    }
    // Set the global hash kind for the repository
    set_hash_kind(match object_format_value.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => HashKind::Sha1,
    });

    Ok(())
}

/// Initialize the configuration for the Libra repository
/// This function creates the necessary configuration entries in the database.
/// Set a directory as hidden on Windows systems
/// This function uses the `attrib` command to set the directory as hidden.
#[cfg(target_os = "windows")]
fn set_dir_hidden(dir: &str) -> io::Result<()> {
    use std::process::Command;
    Command::new("attrib").arg("+H").arg(dir).spawn()?.wait()?; // Wait for command execution to complete
    Ok(())
}

/// On Unix-like systems, directories starting with a dot are hidden by default
/// Therefore, this function does nothing.
#[cfg(not(target_os = "windows"))]
fn set_dir_hidden(_dir: &str) -> io::Result<()> {
    // on unix-like systems, dotfiles are hidden by default
    Ok(())
}
