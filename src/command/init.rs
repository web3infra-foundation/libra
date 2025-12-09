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

use sea_orm::{ActiveModelTrait, DbConn, DbErr, Set, TransactionTrait};

use clap::Parser;

use crate::command::branch;
use crate::internal::db;
use crate::internal::model::{config, reference};
use crate::utils::util::{DATABASE, ROOT_DIR};

const DEFAULT_BRANCH: &str = "master";

#[derive(Parser, Debug, Clone)]
pub struct InitArgs {
    /// 创建一个裸仓库
    #[clap(long, required = false)]
    pub bare: bool, // 默认值是 false

    /// 用于创建仓库的模板目录
    #[clap(long = "template", name = "template-directory", required = false)]
    pub template: Option<String>,

    /// 设置初始分支名称
    #[clap(short = 'b', long, required = false)]
    pub initial_branch: Option<String>,

    /// 在指定目录下创建仓库
    #[clap(default_value = ".")]
    pub repo_directory: String,

    /// 抑制所有输出
    #[clap(long, short = 'q', required = false)]
    pub quiet: bool,

    /// 指定仓库共享模式
    /// 支持值: `umask`, `group`, `all`
    #[clap(long, required = false, value_name = "MODE")]
    pub shared: Option<String>,

    /// 指定仓库对象格式（哈希算法）
    /// 支持值: `sha1`, `sha256`
    #[clap(long = "object-format", name = "format", required = false)]
    pub object_format: Option<String>,

    /// 指定一个独立的 git 目录
    #[clap(long = "separate-git-dir", value_name = "PATH", required = false)]
    pub separate_git_dir: Option<String>,
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
    // 获取当前目录
    let cur_dir = Path::new(&args.repo_directory).to_path_buf();

    // 处理 --separate-git-dir 参数
    let root_dir = match &args.separate_git_dir {
        Some(separate_git_dir) => {
            let separate_git_path = Path::new(separate_git_dir);
            if !separate_git_path.exists() {
                fs::create_dir_all(separate_git_path)?; // 如果指定的 Git 目录不存在，则创建
            }
            separate_git_path.to_path_buf() // 使用指定的目录
        }
        None => {
            if args.bare {
                cur_dir.clone()
            } else {
                cur_dir.join(ROOT_DIR)
            }
        }
    };

    // 检查仓库是否已经初始化
    if is_reinit(&cur_dir) {
        if !args.quiet {
            eprintln!("已经初始化 - [{}]", root_dir.display());
        }
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "初始化失败：指定的位置已经初始化过仓库。如果需要重新初始化，请删除现有的目录或文件。",
        ));
    }

    // 检查目标目录是否可写
    match is_writable(&cur_dir) {
        Ok(_) => {}
        Err(e) => {
            return Err(e);
        }
    }

    // 确保根目录存在
    fs::create_dir_all(&root_dir)?;

    // 如果提供了模板路径，则复制模板文件到根目录
    if let Some(template_path) = &args.template {
        let template_dir = Path::new(template_path);
        if template_dir.exists() {
            copy_template(template_dir, &root_dir)?; // 复制模板内容
        } else {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("模板目录 '{}' 不存在", template_path),
            ));
        }
    } else {
        // 创建仓库相关的目录和文件
        let dirs = ["info", "hooks"];
        for dir in dirs {
            fs::create_dir_all(root_dir.join(dir))?; // 创建 info 和 hooks 目录
        }

        // 创建必要的配置文件
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

        // 设置文件权限
        #[cfg(not(target_os = "windows"))]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o755);
            fs::set_permissions(root_dir.join("hooks").join("pre-commit.sh"), perms)?;
        }

        // 创建 .libra 相关目录
        let dirs = ["objects/pack", "objects/info"];
        for dir in dirs {
            fs::create_dir_all(root_dir.join(dir))?;
        }
    }

    // 创建数据库
    let conn;
    let database = root_dir.join(DATABASE);

    #[cfg(target_os = "windows")]
    {
        // Windows 系统需要转换路径格式
        let database = database.to_str().unwrap().replace("\\", "/");
        conn = db::create_database(database.as_str()).await?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        conn = db::create_database(database.to_str().unwrap()).await?;
    }

    // 初始化配置
    init_config(&conn, args.bare, Some(object_format_value.as_str()))
        .await
        .unwrap();

    // 设置默认的初始分支名称
    let initial_branch_name = args
        .initial_branch
        .unwrap_or_else(|| DEFAULT_BRANCH.to_owned());

    // 创建 HEAD 引用
    reference::ActiveModel {
        name: Set(Some(initial_branch_name.clone())),
        kind: Set(reference::ConfigKind::Head),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .unwrap();

    // 设置 .libra 为隐藏文件夹
    set_dir_hidden(root_dir.to_str().unwrap())?;

    // 如果指定了共享权限，应用共享设置
    if let Some(shared_mode) = &args.shared {
        apply_shared(&root_dir, shared_mode)?;
    }

    if !args.quiet {
        let repo_type = if args.bare { "bare " } else { "" };
        println!(
            "正在初始化空的 {repo_type}Libra 仓库于 {}，初始分支为 '{initial_branch_name}'",
            root_dir.display()
        );
    }

    // 设置全局哈希算法
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
