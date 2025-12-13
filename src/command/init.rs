//! This module implements the `init` command for the Libra CLI.
//!
//! The `init` command creates a new Libra repository in the current directory or a specified directory.
//! It supports customizing the initial branch name with the `--initial-branch` parameter.
//!
use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
};

use sea_orm::{ActiveModelTrait, DbConn, Set, TransactionTrait};
use clap::Parser;

use crate::command::branch;
use crate::internal::db;
use crate::internal::hash::{HashKind, set_hash_kind};
use crate::internal::model::{config, reference};
use crate::utils::util::{DATABASE, ROOT_DIR};

const DEFAULT_BRANCH: &str = "master";

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
    
    /// Specify a separate directory for Git storage
    #[clap(long = "separate-git-dir", value_name = "PATH", required = false)]
    pub separate_git_dir: Option<String>,
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

/// Check if the repository has already been initialized based on the presence of the description file.
fn is_reinit(cur_dir: &Path) -> bool {
    let bare_head_path = cur_dir.join("description");
    let head_path = cur_dir.join(".libra/description");
    // Check the presence of the description file
    head_path.exists() || bare_head_path.exists()
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
#[allow(dead_code)]
pub async fn init(args: InitArgs) -> io::Result<()> {
    // Get current directory
    let cur_dir = Path::new(&args.repo_directory).to_path_buf();

    // Handle --separate-git-dir parameter
    let root_dir = match &args.separate_git_dir {
        Some(separate_git_dir) => {
            let separate_git_path = Path::new(separate_git_dir);
            if !separate_git_path.exists() {
                fs::create_dir_all(separate_git_path)?; // Create the directory if it doesn't exist
            }
            separate_git_path.to_path_buf() // Use the specified directory
        }
        None => {
            if args.bare {
                cur_dir.clone()
            } else {
                cur_dir.join(ROOT_DIR)
            }
        }
    };

    // Check if the repository is already initialized
    if is_reinit(&cur_dir) || (args.separate_git_dir.is_some() && is_reinit(&root_dir)) {
        if !args.quiet {
            eprintln!("Already initialized - [{}]", root_dir.display());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Initialization failed: The repository is already initialized at the specified location.\nIf you wish to reinitialize, please remove the existing directory or file.",
        ));
    }

    // Check if the branch name is valid
    if let Some(ref branch_name) = args.initial_branch
        && !branch::is_valid_git_branch_name(branch_name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "invalid branch name: '{branch_name}'.\n\nBranch names must:\n- Not contain spaces, control characters, or any of these characters: \\ : \" ? * [\n- Not start or end with a slash ('/'), or end with a dot ('.')\n- Not contain consecutive slashes ('//') or dots ('..')\n- Not be reserved names like 'HEAD' or contain '@{{'\n- Not be empty or just a dot ('.')\n\nPlease choose a valid branch name."
            ),
        ));
    }

    // Check if the target directory is writable
    match is_writable(&cur_dir) {
        Ok(_) => {}
        Err(e) => {
            return Err(e);
        }
    }

    // Ensure root directory exists
    fs::create_dir_all(&root_dir)?;
    
    // When using --separate-git-dir, create a .git file in the working directory that points to the actual git directory
    if args.separate_git_dir.is_some() && !args.bare {
        let separate_git_path = Path::new(args.separate_git_dir.as_ref().unwrap());
        let gitlink_path = cur_dir.join(".git");
        let gitlink_content = format!("gitdir: {}", separate_git_path.display());
        fs::write(gitlink_path, gitlink_content)?;
    }

    // Validate and set object_format_value
    let object_format_value = if let Some(format) = &args.object_format {
        match format.to_lowercase().as_str() {
            "sha1" => "sha1".to_string(),
            "sha256" => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "fatal: object format 'sha256' is not supported yet",
                ));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("fatal: invalid object format '{format}'"),
                ));
            }
        }
    } else {
        "sha1".to_string()
    };

    // If a template path is provided, copy template files to the root directory
    if let Some(template_path) = &args.template {
        let template_dir = Path::new(template_path);
        if template_dir.exists() {
            copy_template(template_dir, &root_dir)?; // Copy template content
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Template directory '{}' not found", template_path),
            ));
        }
    } else {
        // Create repository-related directories and files
        let dirs = ["info", "hooks"];
        for dir in dirs {
            fs::create_dir_all(root_dir.join(dir))?; // Create info and hooks directories
        }

        // Create necessary configuration files
        fs::write(
            root_dir.join("info/exclude"),
            include_str!("../../template/exclude"),
        )?;
        fs::write(
            root_dir.join("description"),
            include_str!("../../template/description"),
        )?;
        fs::write(
            root_dir.join("hooks").join("pre-commit.sh"),
            include_str!("../../template/pre-commit.sh"),
        )?;

        // Set file permissions
        #[cfg(not(target_os = "windows"))]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(root_dir.join("hooks").join("pre-commit.sh"), perms)?;
        }

        // Create Windows PowerShell pre-commit hook
        fs::write(
            root_dir.join("hooks").join("pre-commit.ps1"),
            include_str!("../../template/pre-commit.ps1"),
        )?;
    }

    // Create .libra related directories (always create regardless of template)
    let dirs = ["objects/pack", "objects/info"];
    for dir in dirs {
        fs::create_dir_all(root_dir.join(dir))?;
    }

    // Create database
    let conn;
    let database = root_dir.join(DATABASE);

    #[cfg(target_os = "windows")]
    {
        // On Windows, we need to convert the path to a UNC path
        let database = database.to_str().unwrap().replace("\\", "/");
        conn = db::create_database(database.as_str()).await?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        conn = db::create_database(database.to_str().unwrap()).await?;
    }

    // Initialize configuration
    init_config(&conn, args.bare, Some(object_format_value.as_str()))
        .await
        .unwrap();

    // Set default initial branch name
    let initial_branch_name = args
        .initial_branch
        .unwrap_or_else(|| DEFAULT_BRANCH.to_owned());

    // Create HEAD reference
    reference::ActiveModel {
        name: Set(Some(initial_branch_name.clone())),
        kind: Set(reference::ConfigKind::Head),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();

    // Set .libra as hidden folder
    set_dir_hidden(root_dir.to_str().unwrap())?;

    // If shared permissions are specified, apply shared settings
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

    // Set global hash algorithm
    set_hash_kind(match object_format_value.as_str() {
        "sha1" => HashKind::Sha1,
        "sha256" => HashKind::Sha256,
        _ => HashKind::Sha1,
    });

    Ok(())
}

/// Initialize the configuration for the Libra repository
/// This function creates the necessary configuration entries in the database.
async fn init_config(
    conn: &DbConn,
    is_bare: bool,
    object_format: Option<&str>,
) -> sea_orm::DbErr {
    // Begin a new transaction
    let txn = conn.begin().await?;

    // Define the configuration entries for non-Windows systems
    #[cfg(not(target_os = "windows"))]
    let entries = [
        ("repositoryformatversion", "0"),
        ("filemode", "true"),
        ("bare", if is_bare { "true" } else { "false" }),
        ("logallrefupdates", "true"),
    ];

    // Define the configuration entries for Windows systems
    #[cfg(target_os = "windows")]
    let entries = [
        ("repositoryformatversion", "0"),
        ("filemode", "false"), // no filemode on windows
        ("bare", if is_bare { "true" } else { "false" }),
        ("logallrefupdates", "true"),
        ("symlinks", "false"),  // no symlinks on windows
        ("ignorecase", "true"), // ignorecase on windows
    ];

    // Insert each configuration entry into the database
    for (key, value) in entries {
        // tip: Set(None) == NotSet == default == NULL
        let entry = config::ActiveModel {
            configuration: Set("core".to_owned()),
            key: Set(key.to_owned()),
            value: Set(value.to_owned()),
            ..Default::default() // id & name NotSet
        };
        entry.insert(&txn).await?;
    }
    // Insert the object format, defaulting to "sha1" if not specified.
    let object_format_entry = config::ActiveModel {
        configuration: Set("core".to_owned()),
        key: Set("objectformat".to_owned()),
        value: Set(object_format.unwrap_or("sha1").to_owned()),
        ..Default::default() // id & name NotSet
    };
    object_format_entry.insert(&txn).await?;
    // Commit the transaction
    txn.commit().await?;
    Ok(())
}

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