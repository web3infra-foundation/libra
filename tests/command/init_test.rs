//! Initializes a repository by creating .libra storage, seeding HEAD and default refs/config, and preparing the backing database.

use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
};

use clap::{Parser, ValueEnum};
use git_internal::hash::{HashKind, set_hash_kind};
use sea_orm::{ActiveModelTrait, DbConn, DbErr, Set, TransactionTrait};

use crate::{
    internal::{
        db,
        model::{config, reference},
    },
    utils::util::{DATABASE, ROOT_DIR},
};
const DEFAULT_BRANCH: &str = "master";

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
/// Validate branch name according to the specified ref format mode
fn validate_branch_name(branch_name: &str, ref_format: &RefFormat) -> io::Result<()> {
    match ref_format {
        RefFormat::Strict => validate_strict_branch_name(branch_name),
        RefFormat::Filesystem => validate_filesystem_branch_name(branch_name),
    }
}

/// Validate branch name with strict Git-compatible rules
fn validate_strict_branch_name(branch_name: &str) -> io::Result<()> {
    if branch_name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name cannot be empty",
        ));
    }

    if branch_name == "HEAD" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name cannot be 'HEAD'",
        ));
    }

    if branch_name == "@" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name cannot be '@'",
        ));
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    // Cannot start or end with '/'
    if branch_name.starts_with('/') || branch_name.ends_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    // Cannot contain consecutive slashes
    if branch_name.contains("//") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    // Cannot contain ".."
    if branch_name.contains("..") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    // Cannot end with ".lock"
    if branch_name.ends_with(".lock") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    // Cannot end with "."
    if branch_name.ends_with('.') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    Ok(())
}

/// Validate branch name with filesystem-friendly rules
fn validate_filesystem_branch_name(branch_name: &str) -> io::Result<()> {
    if branch_name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name cannot be empty",
        ));
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains filesystem-invalid characters",
        ));
    }

    // Cannot be "." or ".."
    if branch_name == "." || branch_name == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "branch name contains invalid characters",
        ));
    }

    Ok(())
}

#[allow(dead_code)]
pub async fn init(args: InitArgs) -> io::Result<()> {
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unsupported object format: '{}'. Supported formats are 'sha1' and 'sha256'.",
                object_format_value
            ),
        ));
    }

    // Check if the root directory already exists
    if is_reinit(&cur_dir) {
        if !args.quiet {
            eprintln!("Already initialized - [{}]", root_dir.display());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Initialization failed: The repository is already initialized at the specified location.
            If you wish to reinitialize, please remove the existing directory or file.",
        ));
    }

    // Check if the target directory is writable
    match is_writable(&cur_dir) {
        Ok(_) => {}
        Err(e) => {
            return Err(e);
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
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("template directory '{}' does not exist", template_path),
            ));
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
        // Create .libra/description
        fs::write(
            root_dir.join("description"),
            include_str!("../../template/description"),
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

    // Create database: .libra/libra.db
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
        // On Unix-like systems, we do no more
        conn = db::create_database(database.to_str().unwrap()).await?;
    }

    // Create config table with bare parameter consideration and store ref format
    init_config(
        &conn,
        args.bare,
        Some(object_format_value.as_str()),
        args.ref_format.as_ref(),
    )
    .await
    .unwrap();
    // Create config table with bare parameter consideration and store ref format
    init_config(
        &conn,
        args.bare,
        Some(object_format_value.as_str()),
        args.ref_format.as_ref(),
    )
    .await
    .unwrap();

    // Determine the initial branch name: use provided name or default
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

    // Create HEAD (store the branch name as before; ref format stored in config)
    reference::ActiveModel {
        name: Set(Some(initial_branch_name.clone())),
        kind: Set(reference::ConfigKind::Head),
        ..Default::default() // all others are `NotSet`
    }
    .insert(&conn)
    .await
    .unwrap();

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
async fn init_config(
    conn: &DbConn,
    is_bare: bool,
    object_format: Option<&str>,
    ref_format: Option<&RefFormat>,
) -> Result<(), DbErr> {
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
    // Insert the initial ref format used during init
    let ref_format_value = match ref_format {
        Some(RefFormat::Strict) => "strict",
        Some(RefFormat::Filesystem) => "filesystem",
        None => "strict", // default
    };
    let init_ref_format_entry = config::ActiveModel {
        configuration: Set("core".to_owned()),
        key: Set("initrefformat".to_owned()),
        value: Set(ref_format_value.to_owned()),
        ..Default::default()
    };
    init_ref_format_entry.insert(&txn).await?;
    // Insert the initial ref format used during init
    let ref_format_value = match ref_format {
        Some(RefFormat::Strict) => "strict",
        Some(RefFormat::Filesystem) => "filesystem",
        None => "strict", // default
    };
    let init_ref_format_entry = config::ActiveModel {
        configuration: Set("core".to_owned()),
        key: Set("initrefformat".to_owned()),
        value: Set(ref_format_value.to_owned()),
        ..Default::default()
    };
    init_ref_format_entry.insert(&txn).await?;
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